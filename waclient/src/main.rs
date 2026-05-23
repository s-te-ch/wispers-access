use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Mutex, OnceCell};
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
    unsafe {
        libc::umask(0o077);
    }
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

async fn join(share: &str, invite_code: &str) -> Result<()> {
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
    let mut nodes = Vec::new();
    for share in &shares {
        let node = load_node(share).await?;
        nodes.push((share.clone(), node));
    }
    let _sf = StreamFactory::new(nodes);

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

struct StreamFactory {
    nodes: HashMap<String, wc::Node>,
    pool: Mutex<HashMap<String, PoolEntry>>,
}

impl StreamFactory {
    fn new(nodes: Vec<(String, wc::Node)>) -> Self {
        Self {
            nodes: nodes.into_iter().collect(),
            pool: Mutex::new(HashMap::new()),
        }
    }

    async fn open_stream(&self, share: &str) -> Result<wc::QuicStream> {
        let Some(node) = self.nodes.get(share) else {
            anyhow::bail!("Unknown share {}", share);
        };
        // Open a stream with a single retry. This covers the case when the
        // connection has died and needed reestablishing.
        match self.try_open_stream(share, &node).await {
            Ok(s) => Ok(s),
            Err(_) => self.try_open_stream(share, &node).await,
        }
    }

    async fn try_open_stream(&self, share: &str, node: &wc::Node) -> Result<wc::QuicStream> {
        // Get the cell under lock and update last_used.
        let cell = {
            let mut pool = self.pool.lock().await;
            let pool_entry = pool.entry(share.to_owned()).or_insert_with(|| PoolEntry {
                cell: Arc::new(OnceCell::new()),
                last_used: Instant::now(),
            });
            pool_entry.last_used = Instant::now();
            pool_entry.cell.clone()
        };
        // Get or establish the connection.
        let conn = cell
            .get_or_try_init(|| async { node.connect_quic(1).await.map(Arc::new) })
            .await?
            .clone();
        // Open a stream. If this fails, the underlying connection has
        // broken and we should remove it from the pool.
        match conn.open_stream().await {
            Ok(stream) => Ok(stream),
            Err(e) => {
                let mut pool = self.pool.lock().await;
                if let Some(entry) = pool.get(share)
                    && Arc::ptr_eq(&entry.cell, &cell)
                {
                    pool.remove(share);
                }
                Err(e.into())
            }
        }
    }
}

struct PoolEntry {
    cell: Arc<OnceCell<Arc<wc::QuicConnection>>>,
    // TODO: The pool currently doesn't actually use this. Decide whether to
    // expire connections actively, or just let the die and clean them up next
    // time opening a stream fails.
    last_used: Instant,
}
