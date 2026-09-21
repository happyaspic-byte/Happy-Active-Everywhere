use crate::{identity::Identity, share::{self, Record, Share}, transport::{Approval, receive_object, send_object}, versions::Mode, wire::{self, Timing}};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{io::Write, net::SocketAddr, path::{Path, PathBuf}, sync::Arc};
use tokio::{io::{AsyncRead, AsyncWrite}, net::{TcpListener, TcpStream}, time::timeout};
use tokio_rustls::{TlsAcceptor, TlsConnector};

const PROTOCOL: &[u8] = b"everywhere/sync/1";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Hello { folder: String, epoch: String, sequence: u64, seen_epoch: String, seen_sequence: u64, mode: Mode }
#[derive(Serialize, Deserialize)]
#[serde(tag="message", content="records", rename_all="kebab-case")]
enum Metadata { Page(Vec<Record>), End }

fn approval(state: &Path, folder: &str, peer: &str, certificates: &[rustls::pki_types::CertificateDer<'static>], writing: bool) -> Result<Approval> {
    let identity = Identity::new(state, peer)?;
    let certificates = certificates.to_vec(); let state = state.to_owned(); let folder = folder.to_owned(); let peer = peer.to_owned();
    Ok(Arc::new(move || { identity.check(Some(&certificates))?; share::check_access(&state, &folder, &peer, writing) }))
}
async fn send_metadata<S: AsyncWrite + Unpin>(stream: &mut S, share: &Share, local: &Hello, remote: &Hello, check: &Approval) -> Result<()> {
    if local.mode != Mode::ReceiveOnly && remote.mode != Mode::SendOnly {
        let mut since = if remote.seen_epoch == local.epoch { remote.seen_sequence } else { 0 };
        ensure!(since <= local.sequence, "peer cursor exceeds local sequence; index rollback requires a new epoch");
        loop {
            check()?;
            let page = share.records(since, local.sequence, 256)?;
            if page.is_empty() { break; }
            since = page.last().unwrap().seq;
            wire::send(stream, Metadata::Page(page), Timing::default()).await?;
        }
    }
    wire::send(stream, Metadata::End, Timing::default()).await
}
async fn read_metadata<S: AsyncRead + Unpin>(stream: &mut S, share: &Share, peer: &str, remote: &Hello, check: &Approval) -> Result<()> {
    let mut last = 0;
    loop {
        check()?;
        match wire::read(stream, Timing::default()).await? {
            Metadata::Page(page) => {
                ensure!(!page.is_empty() && page.len() <= 256, "invalid metadata page");
                for record in &page { ensure!(record.seq > last && record.seq <= remote.sequence, "unordered metadata sequence"); last = record.seq; }
                share.stage(peer, &page)?;
            }
            Metadata::End => break,
        }
    }
    Ok(())
}
async fn pull_objects<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, share: &Share, check: &Approval) -> Result<()> {
    loop {
        let missing = share.missing_objects(16)?;
        wire::send(stream, &missing, Timing::default()).await?;
        if missing.is_empty() { return Ok(()); }
        for hash in missing {
            receive_object(stream, &share.object_path(&hash)?, &hash, check.clone()).await?;
            share.object_received(&hash)?;
        }
    }
}
async fn serve_objects<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, share: &Share, check: &Approval) -> Result<()> {
    loop {
        let requested: Vec<String> = wire::read(stream, Timing::default()).await?;
        ensure!(requested.len() <= 16, "too many requested objects");
        if requested.is_empty() { return Ok(()); }
        for hash in requested {
            check()?;
            send_object(stream, &share.object_path(&hash)?, &hash, check.clone()).await?;
        }
    }
}
struct Session { state: PathBuf, folder: String, peer: String, read: Approval, write: Approval, client: bool }
async fn session<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S, options: Session) -> Result<()> {
    let timing = Timing::default(); options.read.as_ref()()?;
    let state = options.state.clone(); let folder = options.folder.clone(); let peer = options.peer.clone();
    let share = wire::work(stream, timing, move || {
        let share = Share::open(&state, &folder)?;
        share.authorize(&peer, false)?;
        share.scan(false)?;
        share.begin_incoming()?;
        Ok(share)
    }).await?;
    let (seen_epoch, seen_sequence) = share.cursor(&options.peer)?;
    let local = Hello { folder: options.folder.clone(), epoch: share.config.epoch.clone(), sequence: share.sequence()?, seen_epoch, seen_sequence, mode: share.config.mode };
    wire::send(stream, &local, timing).await?;
    let remote: Hello = wire::read(stream, timing).await?;
    ensure!(remote.folder == local.folder && remote.epoch.len() == 64 && remote.sequence <= i64::MAX as u64, "invalid share handshake");
    if options.client {
        send_metadata(stream, &share, &local, &remote, &options.read).await?;
        read_metadata(stream, &share, &options.peer, &remote, &options.read).await?;
        serve_objects(stream, &share, &options.read).await?;
        pull_objects(stream, &share, &options.write).await?;
    } else {
        read_metadata(stream, &share, &options.peer, &remote, &options.read).await?;
        send_metadata(stream, &share, &local, &remote, &options.read).await?;
        pull_objects(stream, &share, &options.write).await?;
        serve_objects(stream, &share, &options.read).await?;
    }
    let peer = options.peer.clone();
    let share = wire::work(stream, timing, move || {
        if share.config.mode != Mode::SendOnly { share.commit_incoming(&peer)?; }
        Ok(share)
    }).await?;
    (options.read)()?;
    wire::send(stream, "applied", timing).await?;
    let applied: String = wire::read(stream, timing).await?;
    ensure!(applied == "applied", "peer did not apply changes");
    share.set_cursor(&options.peer, &remote.epoch, remote.sequence)?;
    Ok(())
}
pub async fn connect(state: &Path, folder: &str, peer: &str, address: SocketAddr) -> Result<()> {
    share::check_access(state, folder, peer, false)?;
    let identity = Identity::new(state, peer)?; let timing = Timing::default();
    let tcp = timeout(timing.io, TcpStream::connect(address)).await??; tcp.set_nodelay(true)?;
    let mut stream = timeout(timing.io, TlsConnector::from(identity.client_protocol(PROTOCOL)?).connect("everywhere.local".try_into()?, tcp)).await??;
    identity.check(stream.get_ref().1.peer_certificates())?;
    ensure!(stream.get_ref().1.alpn_protocol() == Some(PROTOCOL), "folder protocol mismatch");
    let certs = stream.get_ref().1.peer_certificates().unwrap();
    let options = Session { state: state.into(), folder: folder.into(), peer: peer.into(), read: approval(state,folder,peer,certs,false)?, write: approval(state,folder,peer,certs,true)?, client: true };
    session(&mut stream, options).await
}
pub async fn serve(state: &Path, folder: &str, peer: &str, listen: SocketAddr, once: bool) -> Result<()> {
    share::check_access(state, folder, peer, false)?;
    let identity = Identity::new(state, peer)?; let timing = Timing::default();
    let listener = TcpListener::bind(listen).await?;
    println!("LISTEN {}", listener.local_addr()?); std::io::stdout().flush()?;
    loop {
        let (tcp, _) = listener.accept().await?; tcp.set_nodelay(true)?;
        let outcome = async {
            let mut stream = timeout(timing.io, TlsAcceptor::from(identity.server_protocol(PROTOCOL)?).accept(tcp)).await??;
            identity.check(stream.get_ref().1.peer_certificates())?;
            ensure!(stream.get_ref().1.alpn_protocol() == Some(PROTOCOL), "folder protocol mismatch");
            let certs = stream.get_ref().1.peer_certificates().unwrap();
            let options = Session { state: state.into(), folder: folder.into(), peer: peer.into(), read: approval(state,folder,peer,certs,false)?, write: approval(state,folder,peer,certs,true)?, client: false };
            session(&mut stream, options).await
        }.await;
        if once { return outcome; }
        if let Err(error) = outcome { eprintln!("synchronization rejected: {error:#}"); }
    }
}
