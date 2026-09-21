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
    /// Recover into a NEW workspace with authority disabled until reviewed.
    DeviceRecover {
        #[arg(long)]
        backup: PathBuf,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Restore this identity only when its original device is retired.
        #[arg(long)]
        retired_device: Option<String>,
    },
    /// Restore a recovered folder's prior mode after full peer reconciliation.
    DeviceActivate {
        #[command(flatten)]
        share: ShareArgs,
        /// Accept stale offline state as authoritative without reconciliation.
        #[arg(long)]
        offline_authority: bool,
    },
    /// Generate an offline backup key; print only its public age recipient.
    DeviceKeygen {
        #[arg(long)]
        output: PathBuf,
    },
    /// Encrypt a complete device snapshot; stop active services first.
    DeviceBackup {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        recipient: String,
        #[arg(long)]
        output: PathBuf,
    },
    /// Verify/decrypt a backup and display its nonsecret contents summary.
    DeviceInspect {
        #[arg(long)]
        backup: PathBuf,
        #[arg(long)]
        key: PathBuf,
    },
    /// Register and control the current user's background synchronization service.
    Service {
        #[command(subcommand)]
        action: ServiceCommand,
    },
    /// Serve the private, local management dashboard.
    Manage {
        #[arg(long, hide = true)]
        parent_watch: bool,
        #[arg(long)]
        state: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7445")]
        listen: SocketAddr,
    },
    /// Explicitly display the local management login credential.
    ManagementToken {
        #[arg(long)]
        state: PathBuf,
    },
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
    /// Save a verified folder-state checkpoint with all retained content objects.
    ShareBackup {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        output: PathBuf,
    },
    /// Recover a folder index from a checkpoint without overwriting user files.
    ShareRecover {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        backup: PathBuf,
    },
    /// List concurrent revisions and their preserved content objects.
    ShareConflicts(ShareArgs),
    /// Choose a current conflict revision and propagate the resolution.
    ShareResolve {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        path: String,
        #[arg(long)]
        revision: String,
    },
    /// List locally retained revisions for a relative path.
    ShareHistory {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        path: String,
    },
    /// Restore a retained revision as a new synchronized change.
    ShareRestore {
        #[command(flatten)]
        share: ShareArgs,
        #[arg(long)]
        path: String,
        #[arg(long)]
        revision: String,
    },
    /// Exchange folder changes with one explicitly approved device.
    Sync {
        #[arg(long, hide = true)]
        parent_watch: bool,
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
        #[arg(long, hide = true)]
        parent_watch: bool,
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
#[derive(Subcommand)]
enum ServiceCommand {
    /// Register, enable and start a service using a verified installer prefix.
    Install {
        #[arg(long)]
        state: PathBuf,
        #[arg(long)]
        prefix: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7445")]
        listen: SocketAddr,
    },
    /// Enable login startup and start the service.
    Start(ServiceArgs),
    /// Stop the service and disable login startup until started again.
    Stop(ServiceArgs),
    /// Restart the service using the currently selected installed version.
    Restart(ServiceArgs),
    /// Inspect native registration and authenticated process health.
    Status(ServiceArgs),
    /// Remove the owned registration, preserving device and user data.
    Uninstall(ServiceArgs),
    #[command(hide = true)]
    Run {
        #[arg(long)]
        state: PathBuf,
        #[arg(long, hide = true)]
        parent_watch: bool,
    },
}
#[derive(Args)]
struct ServiceArgs {
    #[arg(long)]
    state: PathBuf,
}
#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Service {
            action:
                ServiceCommand::Install {
                    state,
                    prefix,
                    listen,
                },
        } => {
            println!("{}", everywhere::service::install(&state, &prefix, listen)?);
        }
        Command::Service {
            action:
                ServiceCommand::Run {
                    state,
                    parent_watch,
                },
        } => {
            if parent_watch {
                everywhere::jobs::watch_parent();
            }
            everywhere::service::run(&state)?;
        }
        Command::Service { action } => {
            use everywhere::service::Action;
            let (args, operation) = match action {
                ServiceCommand::Start(args) => (args, Action::Start),
                ServiceCommand::Stop(args) => (args, Action::Stop),
                ServiceCommand::Restart(args) => (args, Action::Restart),
                ServiceCommand::Status(args) => (args, Action::Status),
                ServiceCommand::Uninstall(args) => (args, Action::Uninstall),
                _ => unreachable!(),
            };
            println!("{}", everywhere::service::control(&args.state, operation)?);
        }
        Command::Manage {
            state,
            listen,
            parent_watch,
        } => {
            if parent_watch {
                everywhere::jobs::watch_parent();
            }
            everywhere::management::serve(&state, listen).await?;
        }
        Command::ManagementToken { state } => {
            println!("{}", everywhere::management::token(&state)?)
        }
        Command::ShareInit { share, root, mode } => {
            everywhere::share::Share::create(&share.state, &share.folder, &root, mode)?;
            println!("initialized");
        }
        Command::SharePeer {
            share,
            peer,
            remove,
        } => {
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
        Command::DeviceRecover {
            backup,
            key,
            output,
            retired_device,
        } => println!(
            "{}",
            everywhere::device::recover(&backup, &key, &output, retired_device.as_deref())?
        ),
        Command::DeviceActivate {
            share,
            offline_authority,
        } => println!(
            "{}",
            everywhere::device::activate(&share.state, &share.folder, offline_authority)?
        ),
        Command::DeviceKeygen { output } => println!("{}", everywhere::device::keygen(&output)?),
        Command::DeviceBackup {
            state,
            recipient,
            output,
        } => println!(
            "{}",
            everywhere::device::backup(&state, &recipient, &output)?
        ),
        Command::DeviceInspect { backup, key } => {
            println!("{}", everywhere::device::inspect(&backup, &key)?)
        }
        Command::ShareBackup { share, output } => {
            println!(
                "{}",
                everywhere::share::backup::create(&share.state, &share.folder, &output)?
            );
        }
        Command::ShareRecover { share, backup } => {
            println!(
                "{}",
                everywhere::share::backup::recover(&share.state, &share.folder, &backup)?
            );
        }
        Command::ShareConflicts(share) => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            println!("{}", folder.conflicts()?);
        }
        Command::ShareResolve {
            share,
            path,
            revision,
        } => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            folder.choose(&path, &revision, false)?;
            println!("resolved");
        }
        Command::ShareRestore {
            share,
            path,
            revision,
        } => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            folder.choose(&path, &revision, true)?;
            println!("restored");
        }
        Command::ShareHistory { share, path } => {
            let folder = everywhere::share::Share::open(&share.state, &share.folder)?;
            println!("{}", folder.history(&path)?);
        }
        Command::Sync {
            parent_watch,
            share,
            peer,
            addr,
            continuous,
            interval_ms,
        } => {
            if parent_watch {
                everywhere::jobs::watch_parent();
            }
            loop {
                let result =
                    everywhere::sync::connect(&share.state, &share.folder, &peer, addr).await;
                match result {
                    Ok(()) => println!(
                        "{}",
                        serde_json::json!({"status":"complete","folder":share.folder})
                    ),
                    Err(error) if continuous => eprintln!(
                        "{}",
                        serde_json::json!({"status":"retrying","error":format!("{error:#}")})
                    ),
                    Err(error) => return Err(error),
                }
                if !continuous {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(interval_ms)).await;
            }
        }
        Command::SyncServe {
            parent_watch,
            share,
            peer,
            listen,
            once,
        } => {
            if parent_watch {
                everywhere::jobs::watch_parent();
            }
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
