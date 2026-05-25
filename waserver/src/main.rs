mod http;
mod initialization;
mod ipc;
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
    /// Generates an guest device invite code.
    Invite {
        /// Name of the application share.
        share: String,
        /// Name of the new node.
        node_name: String,
        /// User identification (e.g. email address) of the user.
        user_id: String,
    },
    /// Revoke access.
    Revoke {
        /// Name of the application share.
        share: String,
        /// Node number whose access to revoke.
        node_number: i32,
    },
    /// List guest nodes.
    Nodes {
        /// Name of the application share.
        share: String,
    },
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

    // Parse the command line.
    let cli = Cli::parse();

    // Daemonising must happen before starting tokio.
    match &cli.command {
        Command::Start { .. } => {
            start_daemon()?;
        }
        _ => {}
    }

    // Start async mode.
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
        Command::Start { share, local_port } => serving::serve(&share, local_port).await,
        Command::Stop { share } => stop(&share).await,
        Command::Status => status().await,
        Command::Logs { follow, share } => logs(&follow, &share),
        Command::Invite {
            share,
            node_name,
            user_id,
        } => invite(&share, &node_name, &user_id).await,
        Command::Revoke { share, node_number } => revoke(&share, &node_number).await,
        Command::Nodes { share } => nodes(&share).await,
    }
}

#[cfg(unix)]
fn start_daemon() -> Result<()> {
    let daemonizer = daemonize::Daemonize::new()
        // The daemonize crate defaults to 0o027 post-fork, which would loosen
        // the 0o077 we set in main().
        .umask(0o077);
    daemonizer.start().context("failed to daemonize")?;
    Ok(())
}

#[cfg(windows)]
fn start_daemon(share: &str, local_port: &u16) -> Result<()> {
    use std::fs::{self, File};
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x08000000;

    // Re-launch ourselves with s/start/serve/.
    let exe = std::env::current_exe().context("failed to get current executable path")?;
    let args: Vec<String> = std::iter::once("serve".to_string())
        .chain(std::env::args().skip(2)) // Skip 'waserver start'.
        .collect();

    std::process::Command::new(exe)
        .args(&args)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("failed to spawn background process")?;

    // The background process has been spawned. Exit the starting process.
    std::process::exit(0);
}

async fn stop(share: &str) -> Result<()> {
    let Ok(mut client) = ipc::Client::connect(share).await else {
        anyhow::bail!("cannot connect to server for share {}", share);
    };
    match client.request(&ipc::Request::Shutdown).await {
        Ok(ipc::Response::Success { .. }) => {
            println!("Success!");
        }
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("error stopping server: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    }
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
            Ok(mut client) => {
                match client.request(&ipc::Request::Status).await {
                    Ok(ipc::Response::Success {
                        data: ipc::ResponseData::Status(status),
                        ..
                    }) => {
                        if status.connected_to_hub {
                            format!("serving, local port {}", status.local_port)
                        } else {
                            format!("connecting, local port {}", status.local_port)
                        }
                    }
                    Ok(ipc::Response::Success { .. }) => {
                        anyhow::bail!("unexpected response from server");
                    }
                    Ok(ipc::Response::Error { error, .. }) => {
                        format!("error getting status: {}", error)
                    }
                    Err(_) => {
                        // Probably went down just now, return "offline".
                        "offline".to_string()
                    }
                }
            }
            Err(_) => "offline".to_string(),
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

async fn invite(share: &str, node_name: &str, user_id: &str) -> Result<()> {
    println!("invite({}, {});", share, user_id);
    let Ok(mut client) = ipc::Client::connect(share).await else {
        anyhow::bail!("cannot connect to server for share {}", share);
    };
    let req = ipc::Request::GetInvite {
        node_name: node_name.to_owned(),
        user_id: user_id.to_owned(),
    };
    match client.request(&req).await {
        Ok(ipc::Response::Success {
            data: ipc::ResponseData::Invite(invite),
            ..
        }) => {
            println!("Token: {}", invite.registration_token);
            println!("Code: {}", invite.activation_code);
        }
        Ok(ipc::Response::Success { .. }) => {
            anyhow::bail!("unexpected response from server");
        }
        Ok(ipc::Response::Error { error, .. }) => {
            anyhow::bail!("error generating invite: {}", error);
        }
        Err(e) => {
            anyhow::bail!("error sending command to server: {}", e);
        }
    }
    Ok(())
}

async fn revoke(share: &str, node_number: &i32) -> Result<()> {
    println!("revoke({}, {});", share, node_number);
    // TODO: add revocation support to the library. Right now all it has is logout().
    Ok(())
}

async fn nodes(share: &str) -> Result<()> {
    println!("nodes({});", share);
    Ok(())
}
