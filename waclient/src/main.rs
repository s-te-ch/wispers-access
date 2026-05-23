use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use wispers_connect as wc;

#[derive(Parser)]
#[command(name = "waclient", version)]
#[command(about = "Wispers Access client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Join a Wispers Accept share.
    Join {
        /// Share name.
        share: String,
        /// Invite code for the share, produced by waserver.
        invite_code: String,
    },
    Serve {
        port: u16,
    },
}

fn main() -> Result<()> {
    // Restrict default file mode to user-only. Safe to do as the first thing.
    #[cfg(unix)]
    unsafe { libc::umask(0o077); }
    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Join { share, invite_code } => join(&share, &invite_code).await,
        Command::Serve { port } => serve(port.clone()).await,
    }
}

async fn join(share : &str, invite_code: &str) -> Result<()> {
    let Some((registration_token, activation_code)) = invite_code.split_once('/') else {
        anyhow::bail!("invalid invite code");
    };
    let storage = get_node_storage(share)?;
    if let Some(_) = storage.read_registration()? {
        anyhow::bail!("found existing registration for share {}", share);
    }
    let mut node = storage.restore_or_init_node().await?;
    println!("Registering Wispers node...");
    node.register(registration_token).await?;
    println!("Activating Wispers node...");
    node.activate(activation_code).await?;
    println!(
        "Joined share {}\n  Connectivity group ID: {}\n  Node number: {}\n",
        share,
        node.connectivity_group_id().unwrap().to_string(),
        node.node_number().unwrap(),
    );

    Ok(())
}

async fn serve(_port: u16) -> Result<()> {
    let shares = list_shares()?;
    let mut nodes = HashMap::new();
    for share in &shares {
        let node = load_node(share).await?;
        nodes.insert(share.clone(), node);
    }

    println!("Found shares:");
    for share in &shares {
        println!("  {}", share);
    }
    // TODO:
    // - start a node for each -
    // - keyed connection pool that gets connections from the matching node (based on appname)
    // - Listen on port, dispatch each new client connection
    //   - Get stream from pool
    Ok(())
}

async fn load_node(share: &str) -> Result<wc::Node> {
    let ns = get_node_storage(share)?;
    if !ns.read_registration()?.is_some() {
        anyhow::bail!("Share {} doesn't have a registered node", share);
    }
    let node = ns.restore_or_init_node().await?;
    Ok(node)
}

fn get_node_storage(share: &str) -> Result<wc::NodeStorage> {
    let dir = get_storage_dir()?;
    let dir = dir.join(share);
    let store = wc::FileNodeStateStore::new(dir);
    let storage = wc::NodeStorage::new(store);
    Ok(storage)
}

fn list_shares() -> Result<Vec<String>> {
    let dir = get_storage_dir()?;
    let shares: Vec<String> = if !dir.exists() {
        Vec::new()
    } else {
        fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect()
    };
    Ok(shares)

}

fn get_storage_dir() -> Result<PathBuf> {
    let config_dir = dirs::config_dir().context("could not determine config directory")?;
    Ok(config_dir.join("waclient"))
}
