mod http;
mod initialization;
mod ipc;
mod quic_stream_io;
mod serving;
mod storage;
mod wcbe;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "waserver", version)]
#[command(about = "Wispers Access server")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Initialise a new application share
    Init {
        /// Wispers Connect API key (can also be set via WC_API_KEY env var).
        #[arg(long, env = "WC_API_KEY", hide_env_values = true)]
        api_key: String,
        /// Name of the application share.
        share: String,
    },
    /// De-initialise an existing application share.
    Deinit {
        /// Name of the application share.
        share: String,
    },
    /// Runs the server proxying <name> in the foreground.
    Serve {
        /// Name of the application share.
        share: String,
        /// Local port to proxy.
        local_port: u16,
    },
    /// Runs the server in the background.
    Start {
        /// Name of the application share.
        share: String,
        /// Local port to proxy.
        local_port: u16,
    },
    /// Stops a server that is running in the background.
    Stop {
        /// Name of the application share.
        share: String,
    },
    /// Shows the status of all running servers.
    Status,
    /// Prints the logs of the given server to stdout.
    Logs {
        /// Don't stop at EOF. Instead, wait for more logs to be written.
        #[arg(short = 'f', long)]
        follow: bool,
        share: String,
    },
    // TODO: remove share, invite & revoke guest
}

fn main() -> Result<()> {
    // Restrict default file mode to user-only. None of files waserver writes
    // have an obvious reason to be group- or world-readable. This is marked
    // unsafe because it changes global state, but doing so as the first thing
    // is safe.
    #[cfg(unix)]
    unsafe {
        libc::umask(0o077);
    }
    // De-conflict rustls. reqwest pulls it in via the aws-lc-rs provider
    // feature, and wispers-connect via the ring feature. We have to choose one.
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .expect("install rustls crypto provider");

    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Init { share, api_key } => initialization::up(&api_key, &share).await,
        Command::Deinit { share } => initialization::down(&share).await,
        Command::Serve { share, local_port } => serving::serve(&share, local_port).await,
        Command::Start { share, local_port } => start(&share, &local_port),
        Command::Stop { share } => stop(&share),
        Command::Status => status().await,
        Command::Logs { follow, share } => logs(&follow, &share),
    }
}

fn start(share: &str, local_port: &u16) -> Result<()> {
    println!("start({}, {});", share, local_port);
    // What this needs to do:
    // - Daemonise (see wconnect)
    // - Serve
    Ok(())
}

fn stop(share: &str) -> Result<()> {
    println!("stop({});", share);
    // What this needs to do:
    // - Find the server (UDS on *NIX, port on Windows)
    // - Tell it to shut down
    // (this should work with both foreground and daemon servers)
    Ok(())
}

async fn status() -> Result<()> {
    let shares = storage::list_shares()?;
    if shares.is_empty() {
        println!("No app shares found");
        return Ok(());
    }
    for share in &shares {
        let status = match ipc::Client::connect(share).await {
            Ok(_) => "running",
            Err(_) => "offline",
        };
        println!("{} ({})", share, status);
    }
    Ok(())
}

fn logs(follow: &bool, share: &str) -> Result<()> {
    println!("logs({}, {});", follow, share);
    // - Find the latest logs of the given share (platform dependent)
    // - Print them, or if -f is given, tail them
    Ok(())
}
