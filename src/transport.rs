use crate::{
    identity::{Identity, PROTOCOL},
    storage::{Manifest, Receiver, block_len},
    wire::{self, Timing},
};
use anyhow::{Result, ensure};
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

pub async fn receive(
    state: &Path,
    peer: &str,
    listen: SocketAddr,
    output: &Path,
    once: bool,
) -> Result<()> {
    let identity = Identity::new(state, peer)?;
    let timing = Timing::default();
    let listener = TcpListener::bind(listen).await?;
    println!("LISTEN {}", listener.local_addr()?);
    std::io::stdout().flush()?;
    loop {
        let (tcp, _) = listener.accept().await?;
        tcp.set_nodelay(true)?;
        let result = async {
            let mut stream = timeout(timing.io, TlsAcceptor::from(identity.server()?).accept(tcp)).await??;
            identity.check(stream.get_ref().1.peer_certificates())?;
            ensure!(stream.get_ref().1.alpn_protocol() == Some(PROTOCOL), "protocol mismatch");
            let certificates = stream.get_ref().1.peer_certificates().unwrap().to_vec();
            let manifest: Manifest = wire::read(&mut stream, timing).await?;
            manifest.validate()?;
            let output = output.to_owned();
            let expected = manifest.clone();
            let (mut receiver, missing) = wire::work(&mut stream, timing, move || {
                let mut receiver = Receiver::open(&output, expected)?;
                let missing = receiver.missing()?;
                Ok((receiver, missing))
            }).await?;
            wire::send(&mut stream, &missing, timing).await?;
            for index in &missing {
                identity.check(Some(&certificates))?;
                let received_index = timeout(timing.io, stream.read_u64()).await??;
                ensure!(received_index == *index, "unexpected block index");
                let mut buffer = vec![0; block_len(&manifest, *index)?];
                timeout(timing.io, stream.read_exact(&mut buffer)).await??;
                let index = *index;
                receiver = wire::work(&mut stream, timing, move || {
                    receiver.put(index, &buffer)?;
                    Ok(receiver)
                }).await?;
                wire::send(&mut stream, index, timing).await?;
            }
            let commit: String = wire::read(&mut stream, timing).await?;
            ensure!(commit == "commit", "sender did not commit verified source");
            identity.check(Some(&certificates))?;
            let approval = identity.clone();
            wire::work(&mut stream, timing, move || {
                receiver.finish_checked(|| approval.check(Some(&certificates)))
            }).await?;
            wire::send(&mut stream, "complete", timing).await?;
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

struct Blocks<'a> {
    source: &'a Path,
    manifest: &'a Manifest,
    missing: &'a [u64],
    delay: Duration,
}

async fn send_blocks<S>(
    stream: &mut S,
    blocks: Blocks<'_>,
    check: impl Fn() -> Result<()>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let timing = Timing::default();
    // Keep at most 16 MiB awaiting durable acknowledgement. Pacing retains the
    // single-block contract used for deliberate interruption and recovery.
    let window = tokio::sync::Semaphore::new(if blocks.delay.is_zero() { 16 } else { 1 });
    let (mut reader, mut writer) = tokio::io::split(stream);
    let sending = async {
        let mut source = File::open(blocks.source)?;
        let mut buffer = vec![0; crate::storage::BLOCK_SIZE];
        for index in blocks.missing {
            window.acquire().await?.forget();
            check()?;
            let len = block_len(blocks.manifest, *index)?;
            source.seek(SeekFrom::Start(index * crate::storage::BLOCK_SIZE as u64))?;
            source.read_exact(&mut buffer[..len])?;
            ensure!(
                blake3::hash(&buffer[..len]).to_hex().as_str()
                    == blocks.manifest.blocks[*index as usize],
                "source changed during transfer"
            );
            // The concurrent ACK reader enforces peer inactivity while this
            // write can be backpressured by a slow, heartbeating disk worker.
            timeout(timing.operation, async {
                writer.write_u64(*index).await?;
                writer.write_all(&buffer[..len]).await?;
                writer.flush().await
            })
            .await??;
            if !blocks.delay.is_zero() {
                tokio::time::sleep(blocks.delay).await;
            }
        }
        Ok::<(), anyhow::Error>(())
    };
    let receiving = async {
        for index in blocks.missing {
            let acknowledged: u64 = wire::read(&mut reader, timing).await?;
            check()?;
            ensure!(acknowledged == *index, "unexpected acknowledgement");
            window.add_permits(1);
            eprintln!("BLOCK {index}");
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::try_join!(sending, receiving)?;
    Ok(())
}

pub async fn send(
    state: &Path,
    peer: &str,
    addr: SocketAddr,
    source: &Path,
    block_delay_ms: u64,
) -> Result<()> {
    let identity = Identity::new(state, peer)?;
    let timing = Timing::default();
    let manifest = Manifest::from_path(source)?;
    let tcp = timeout(timing.io, TcpStream::connect(addr)).await??;
    tcp.set_nodelay(true)?;
    let mut stream = timeout(
        timing.io,
        TlsConnector::from(identity.client()?).connect("everywhere.local".try_into()?, tcp),
    )
    .await??;
    identity.check(stream.get_ref().1.peer_certificates())?;
    ensure!(
        stream.get_ref().1.alpn_protocol() == Some(PROTOCOL),
        "protocol mismatch"
    );
    let certificates = stream.get_ref().1.peer_certificates().unwrap().to_vec();
    wire::send(&mut stream, &manifest, timing).await?;
    let missing: Vec<u64> = wire::read(&mut stream, timing).await?;
    ensure!(
        missing.len() <= manifest.blocks.len(),
        "too many requested blocks"
    );
    ensure!(
        missing.windows(2).all(|p| p[0] < p[1]),
        "block requests must be unique and ordered"
    );
    send_blocks(
        &mut stream,
        Blocks {
            source,
            manifest: &manifest,
            missing: &missing,
            delay: Duration::from_millis(block_delay_ms),
        },
        || identity.check(Some(&certificates)),
    )
    .await?;
    let source = source.to_owned();
    let expected = manifest.clone();
    wire::work(&mut stream, timing, move || {
        ensure!(
            Manifest::from_path(&source)? == expected,
            "source changed before commit"
        );
        Ok(())
    })
    .await?;
    wire::send(&mut stream, "commit", timing).await?;
    let result: String = wire::read(&mut stream, timing).await?;
    ensure!(result == "complete", "receiver did not complete");
    println!(
        "{}",
        serde_json::json!({"status":"complete","tls":"TLSv1_3","protocol":2,"sent_blocks":missing.len(),"reused_blocks":manifest.blocks.len()-missing.len(),"bytes":manifest.size,"hash":manifest.hash})
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::BLOCK_SIZE;

    #[tokio::test]
    async fn two_blocks_can_be_in_flight_before_the_first_acknowledgement() {
        let directory = tempfile::TempDir::new().unwrap();
        let source = directory.path().join("source");
        std::fs::write(&source, vec![7; BLOCK_SIZE * 2]).unwrap();
        let manifest = Manifest::from_path(&source).unwrap();
        let (mut sender, mut receiver) = tokio::io::duplex(BLOCK_SIZE);
        let receive = async {
            for expected in [0, 1] {
                let index = timeout(Duration::from_secs(2), receiver.read_u64()).await??;
                ensure!(index == expected, "wrong block");
                let mut bytes = vec![0; BLOCK_SIZE];
                timeout(Duration::from_secs(2), receiver.read_exact(&mut bytes)).await??;
                ensure!(bytes.iter().all(|b| *b == 7), "wrong bytes");
            }
            for index in [0u64, 1] {
                wire::send(&mut receiver, index, Timing::default()).await?;
            }
            Ok::<(), anyhow::Error>(())
        };
        let send = send_blocks(
            &mut sender,
            Blocks {
                source: &source,
                manifest: &manifest,
                missing: &[0, 1],
                delay: Duration::ZERO,
            },
            || Ok(()),
        );
        // Bound the whole scenario; a stop-and-wait sender deadlocks here.
        let (sent, received) = timeout(Duration::from_secs(5), async {
            tokio::join!(send, receive)
        })
        .await
        .unwrap();
        received.unwrap();
        sent.unwrap();
    }
}

pub(crate) type Approval = std::sync::Arc<dyn Fn() -> Result<()> + Send + Sync>;

pub(crate) async fn send_object<S>(stream: &mut S, source: &Path, hash: &str, check: Approval) -> Result<()>
where S: AsyncRead + AsyncWrite + Unpin {
    let timing = Timing::default();
    check()?;
    let source_path = source.to_owned();
    let manifest = wire::work(stream, timing, move || Manifest::from_path(&source_path)).await?;
    ensure!(manifest.hash == hash, "shared content object is corrupt");
    wire::send(stream, &manifest, timing).await?;
    let missing: Vec<u64> = wire::read(stream, timing).await?;
    ensure!(missing.len() <= manifest.blocks.len() && missing.windows(2).all(|p| p[0] < p[1]), "invalid block request");
    send_blocks(stream, Blocks { source, manifest: &manifest, missing: &missing, delay: Duration::ZERO }, || check()).await?;
    let source_path = source.to_owned();
    let expected = manifest.clone();
    wire::work(stream, timing, move || {
        ensure!(Manifest::from_path(&source_path)? == expected, "shared object changed during transfer");
        Ok(())
    }).await?;
    check()?;
    wire::send(stream, "commit", timing).await?;
    let result: String = wire::read(stream, timing).await?;
    ensure!(result == "complete", "object receiver did not complete");
    Ok(())
}

pub(crate) async fn receive_object<S>(stream: &mut S, destination: &Path, hash: &str, check: Approval) -> Result<()>
where S: AsyncRead + AsyncWrite + Unpin {
    let timing = Timing::default();
    let manifest: Manifest = wire::read(stream, timing).await?;
    manifest.validate()?; ensure!(manifest.hash == hash, "unrequested content object");
    check()?;
    let target = destination.to_owned(); let expected = manifest.clone();
    let (mut receiver, missing) = wire::work(stream, timing, move || {
        let mut receiver = Receiver::open(&target, expected)?;
        let missing = receiver.missing()?; Ok((receiver, missing))
    }).await?;
    wire::send(stream, &missing, timing).await?;
    for index in missing {
        check()?;
        let received = timeout(timing.io, stream.read_u64()).await??;
        ensure!(received == index, "unexpected object block");
        let mut bytes = vec![0; block_len(&manifest, index)?];
        timeout(timing.io, stream.read_exact(&mut bytes)).await??;
        receiver = wire::work(stream, timing, move || { receiver.put(index, &bytes)?; Ok(receiver) }).await?;
        wire::send(stream, index, timing).await?;
    }
    let commit: String = wire::read(stream, timing).await?;
    ensure!(commit == "commit", "uncommitted content object");
    wire::work(stream, timing, move || receiver.finish_checked(|| check())).await?;
    wire::send(stream, "complete", timing).await?;
    Ok(())
}
