use anyhow::Result;
use clap::{Args, Parser, Subcommand};
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
#[derive(Args)]
struct ShareArgs {
    #[arg(long)]
    state: PathBuf,
    #[arg(long)]
    folder: String,
}
#[derive(Subcommand)]
enum Command {
    /// Register a folder and create its independent synchronization index.
    ShareInit {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        root: PathBuf,
        #[arg(long, value_enum, default_value = "bidirectional")]
        mode: everywhere::versions::Mode,
    },
    /// Grant or remove a trusted device's access to one folder.
    SharePeer {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        peer: String,
        #[arg(long)]
        remove: bool,
    },
    /// Record local changes; missing paths remain pending approval.
    ShareScan(ShareArgs),
    /// Approve all currently missing local paths as deletion revisions.
    ShareApproveDeletes {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        all: bool,
    },
    /// Inspect folder versions, pending deletions and the index epoch.
    ShareStatus(ShareArgs),
    /// List concurrent revisions and their preserved content objects.
    ShareConflicts(ShareArgs),
    /// Exchange folder changes with one explicitly approved device.
    Sync {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        peer: String,
        #[arg(long)]
        addr: SocketAddr,
        #[arg(long)]
        continuous: bool,
        #[arg(long, default_value_t = 2000, value_parser = clap::value_parser!(u64).range(250..))]
        interval_ms: u64,
    },
    /// Listen for authenticated changes to one folder from one approved peer.
    SyncServe {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        peer: String,
        #[arg(long, default_value = "127.0.0.1:7444")]
        listen: SocketAddr,
        #[arg(long)]
        once: bool,
    },

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
    /// Detect folder changes with bounded-interval full rescans.
    Watch {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        root: PathBuf,
        #[arg(long)]
        device: String,
        #[arg(long, default_value_t=1000, value_parser=clap::value_parser!(u64).range(50..))]
        interval_ms: u64,
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
        Command::ShareInit { share, root, mode } => {
            everywhere::share::Share::create(&share.state, &share.folder, &root, mode)?;
            println!("initialized");
        }
        Command::SharePeer { share, peer, remove } => {
            everywhere::share::grant(&share.state, &share.folder, &peer, remove)?;
            println!("updated");
        }
        Command::ShareScan(share) => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            println!("{}", serde_json::to_string(&folder.scan(false)?)?);
        }
        Command::ShareApproveDeletes { share, all } => {
            anyhow::ensure!(all, "specify --all to approve the currently missing paths");
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            println!("{}", serde_json::to_string(&folder.scan(true)?)?);
        }
        Command::ShareStatus(share) => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            println!("{}", folder.status()?);
        }
        Command::ShareConflicts(share) => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            println!("{}", folder.conflicts()?);
        }
        Command::Sync { share, peer, addr, continuous, interval_ms } => {
            loop {
                let result = everywhere::sync::connect(&share.state, &share.folder, &peer, addr).await;
                match result {
                    Ok(()) => println!("{}", serde_json::json!({"status":"complete","folder":share.folder})),
                    Err(error) if continuous => eprintln!("{}", serde_json::json!({"status":"retrying","error":format!("{error:#}")})),
                    Err(error) => return Err(error),
                }
                if !continuous { break; }
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
        }
        Command::SyncServe { share, peer, listen, once } => {
            everywhere::sync::serve(&share.state, &share.folder, &peer, listen, once).await?;
        }

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
        Command::Watch {
            db,
            root,
            device,
            interval_ms,
        } => {
            use std::io::Write;
            let mut index = everywhere::index::Index::open(&db, &root, &device)?;
            println!("{}", serde_json::json!({"status":"watching"}));
            std::io::stdout().flush()?;
            loop {
                match index.scan(false) {
                    Ok(events) if !events.is_empty() => {
                        println!("{}", serde_json::json!({"events":events}))
                    }
                    Ok(_) => {}
                    Err(error) => println!("{}", serde_json::json!({"error":format!("{error:#}")})),
                }
                std::io::stdout().flush()?;
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
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
