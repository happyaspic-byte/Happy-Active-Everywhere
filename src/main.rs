use anyhow::Result;
use clap::{Parser, Subcommand};
use everywhere::{identity, transport};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "Authenticated peer-to-peer file transfer — experimental alpha"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Restore preserved file content, saving the current version first.
    Restore {
        #[arg(long)]
        version_file: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Create a local folder index outside the synchronized folder.
    FolderInit {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        device: String,
    },
    /// Rescan contents; missing files require explicit deletion approval.
    Scan {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        device: String,
        #[arg(long)]
        allow_deletes: bool,
    },
    /// Show persisted file versions and deletion records.
    IndexStatus {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        device: String,
    },
    /// Create a new device identity; refuses existing state directories.
    Init {
        #[arg(long)]
        state: PathBuf,
    },
    /// Approve a peer certificate obtained over a trusted channel.
    Trust {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        cert: PathBuf,
    },
    /// Revoke the local approval for a peer.
    Revoke {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        peer: String,
    },
    /// Receive one file at an explicitly authorized local path.
    Receive {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        peer: String,
        #[arg(long, default_value = "127.0.0.1:7443")]
        listen: SocketAddr,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        once: bool,
    },
    /// Send a file; verified blocks from an interrupted transfer are reused.
    Send {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        peer: String,
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long)]
        source: PathBuf,
        /// Optional pacing interval between acknowledged blocks.
        #[arg(long, default_value_t = 0)]
        block_delay_ms: u64,
    },
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Restore {
            version_file,
            output,
        } => {
            everywhere::storage::restore(&version_file, &output)?;
            println!("restored");
        }
        Command::FolderInit { db, root, device } => {
            everywhere::index::Index::create(&db, &root, &device)?;
            println!("initialized");
        }
        Command::Scan {
            db,
            root,
            device,
            allow_deletes,
        } => {
            let events =
                everywhere::index::Index::open(&db, &root, &device)?.scan(allow_deletes)?;
            println!("{}", serde_json::to_string(&events)?);
        }
        Command::IndexStatus { db, root, device } => {
            let entries = everywhere::index::Index::open(&db, &root, &device)?.entries()?;
            println!("{}", serde_json::to_string(&entries)?);
        }
        Command::Init { state } => println!("{}", identity::init(&state)?),
        Command::Trust { state, cert } => println!("{}", identity::trust(&state, &cert)?),
        Command::Revoke { state, peer } => identity::revoke(&state, &peer)?,
        Command::Receive {
            state,
            peer,
            listen,
            output,
            once,
        } => transport::receive(&state, &peer, listen, &output, once).await?,
        Command::Send {
            state,
            peer,
            addr,
            source,
            block_delay_ms,
        } => transport::send(&state, &peer, addr, &source, block_delay_ms).await?,
    }
    Ok(())
}
