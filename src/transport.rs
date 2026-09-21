use crate::{
    identity::Identity,
    storage::{Manifest, Receiver, block_len},
};
use anyhow::{Result, bail, ensure};
use serde::{Serialize, de::DeserializeOwned};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    net::SocketAddr,
    path::Path,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const MAX_FRAME: usize = 80 * 1024 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(60);

async fn send_json<S: AsyncWrite + Unpin, T: Serialize>(stream: &mut S, value: &T) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "frame too large");
    timeout(IO_TIMEOUT, async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.flush().await
    })
    .await??;
    Ok(())
}
async fn read_json<S: AsyncRead + Unpin, T: DeserializeOwned>(stream: &mut S) -> Result<T> {
    let size = timeout(IO_TIMEOUT, stream.read_u32()).await?? as usize;
    ensure!(size <= MAX_FRAME, "frame too large");
    let mut bytes = vec![0; size];
    timeout(IO_TIMEOUT, stream.read_exact(&mut bytes)).await??;
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn receive(
    state: &Path,
    peer: &str,
    listen: SocketAddr,
    output: &Path,
    once: bool,
) -> Result<()> {
    let identity = Identity::new(state, peer)?;
    let listener = TcpListener::bind(listen).await?;
    println!("LISTEN {}", listener.local_addr()?);
    std::io::stdout().flush()?;
    loop {
        let (tcp, _) = listener.accept().await?;
        tcp.set_nodelay(true)?;
        let result = async {
            let mut stream = timeout(IO_TIMEOUT, TlsAcceptor::from(identity.server()?).accept(tcp)).await??;
            identity.check(stream.get_ref().1.peer_certificates())?;
            ensure!(stream.get_ref().1.alpn_protocol() == Some(b"everywhere/1".as_slice()), "protocol mismatch");
            let manifest: Manifest = read_json(&mut stream).await?;
            manifest.validate()?;
            let mut receiver = Receiver::open(output, manifest.clone())?;
            let missing = receiver.missing()?;
            send_json(&mut stream, &missing).await?;
            let mut buffer = vec![0; crate::storage::BLOCK_SIZE];
            for index in &missing {
                identity.check(stream.get_ref().1.peer_certificates())?;
                let received_index = timeout(IO_TIMEOUT, stream.read_u64()).await??;
                ensure!(received_index == *index, "unexpected block index");
                let len = block_len(&manifest, *index)?;
                timeout(IO_TIMEOUT, stream.read_exact(&mut buffer[..len])).await??;
                receiver.put(*index, &buffer[..len])?;
                send_json(&mut stream, index).await?;
            }
            let commit: String = read_json(&mut stream).await?;
            ensure!(commit == "commit", "sender did not commit verified source");
            identity.check(stream.get_ref().1.peer_certificates())?;
            receiver.finish()?;
            send_json(&mut stream, &"complete").await?;
            println!("{}", serde_json::json!({"status":"complete","received_blocks":missing.len(),"reused_blocks":manifest.blocks.len()-missing.len()}));
            Ok::<_, anyhow::Error>(())
        }.await;
        if once {
            return result;
        }
        if let Err(error) = result {
            eprintln!("transfer rejected: {error:#}");
        }
    }
}

pub async fn send(
    state: &Path,
    peer: &str,
    addr: SocketAddr,
    source: &Path,
    block_delay_ms: u64,
) -> Result<()> {
    let identity = Identity::new(state, peer)?;
    let manifest = Manifest::from_path(source)?;
    let mut source_file = File::open(source)?;
    let tcp = timeout(IO_TIMEOUT, TcpStream::connect(addr)).await??;
    tcp.set_nodelay(true)?;
    let mut stream = timeout(
        IO_TIMEOUT,
        TlsConnector::from(identity.client()?).connect("everywhere.local".try_into()?, tcp),
    )
    .await??;
    identity.check(stream.get_ref().1.peer_certificates())?;
    ensure!(
        stream.get_ref().1.alpn_protocol() == Some(b"everywhere/1".as_slice()),
        "protocol mismatch"
    );
    send_json(&mut stream, &manifest).await?;
    let missing: Vec<u64> = read_json(&mut stream).await?;
    ensure!(
        missing.len() <= manifest.blocks.len(),
        "too many requested blocks"
    );
    ensure!(
        missing.windows(2).all(|p| p[0] < p[1]),
        "block requests must be unique and ordered"
    );
    let mut buffer = vec![0; crate::storage::BLOCK_SIZE];
    for index in &missing {
        identity.check(stream.get_ref().1.peer_certificates())?;
        let len = block_len(&manifest, *index)?;
        source_file.seek(SeekFrom::Start(index * crate::storage::BLOCK_SIZE as u64))?;
        source_file.read_exact(&mut buffer[..len])?;
        ensure!(
            blake3::hash(&buffer[..len]).to_hex().as_str() == manifest.blocks[*index as usize],
            "source changed during transfer"
        );
        timeout(IO_TIMEOUT, async {
            stream.write_u64(*index).await?;
            stream.write_all(&buffer[..len]).await?;
            stream.flush().await
        })
        .await??;
        let acknowledged: u64 = read_json(&mut stream).await?;
        ensure!(acknowledged == *index, "unexpected acknowledgement");
        eprintln!("BLOCK {index}");
        if block_delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(block_delay_ms)).await;
        }
    }
    if Manifest::from_path(source)? != manifest {
        bail!("source changed before commit");
    }
    send_json(&mut stream, &"commit").await?;
    let result: String = read_json(&mut stream).await?;
    ensure!(result == "complete", "receiver did not complete");
    println!(
        "{}",
        serde_json::json!({"status":"complete","tls":"TLSv1_3","sent_blocks":missing.len(),"reused_blocks":manifest.blocks.len()-missing.len(),"bytes":manifest.size,"hash":manifest.hash})
    );
    Ok(())
}
