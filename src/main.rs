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
