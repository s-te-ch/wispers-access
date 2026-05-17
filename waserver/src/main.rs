mod storage;
mod wcbe;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use futures::future::BoxFuture;

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
    let cli = Cli::parse();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to create tokio runtime")?
        .block_on(async_main(cli.command))
}

async fn async_main(command: Command) -> Result<()> {
    match command {
        Command::Init { share, api_key } => init(&api_key, &share).await,
        Command::Serve { share, local_port } => serve(&share, &local_port),
        Command::Start { share, local_port } => start(&share, &local_port),
        Command::Stop { share } => stop(&share),
        Command::Status => status(),
        Command::Logs { follow, share } => logs(&follow, &share),
    }
}

//-- init -----------------------------------------------------------------------

type RollbackVec = Vec<BoxFuture<'static, ()>>;

async fn init(api_key: &str, share: &str) -> Result<()> {
    println!("init({}, {});", api_key, share);

    if !is_valid_share_name(share) {
        anyhow::bail!("Invalid share name '{}'", share);
    }
    let store = storage::ShareStateStore::new(share)?;
    if let Some(_) = store.load_share_config()? {
        anyhow::bail!("Share {} already exists", share);
    }
    let wcbe_client = wcbe::Client::new(api_key);

    // Run initialisation while keeping track of rollback steps.
    let mut rollback: RollbackVec = Vec::new();
    let result: Result<()> = async {
        let cg_id = add_connectivity_group(&wcbe_client, &share, &mut rollback).await?;
        store_share_config(&store, &api_key, &mut rollback)?;
        let mut node = create_wispers_node(&store, &mut rollback).await?;
        let token = wcbe_client
            .get_registration_token(&cg_id, "Server".into())
            .await?;
        node.register(&token).await.context("registration failed")?;
        Ok(())
    }
    .await;
    if result.is_err() {
        for action in rollback.into_iter().rev() {
            action.await;
        }
    }
    Ok(())
}

async fn add_connectivity_group(
    client: &wcbe::Client,
    share: &str,
    rollback: &mut RollbackVec,
) -> Result<String> {
    let cg_id = client.add_connectivity_group(share).await?;
    rollback.push(Box::pin({
        let client = client.clone();
        let cg_id = cg_id.clone();
        async move {
            if let Err(e) = client.remove_connectivity_group(&cg_id).await {
                eprintln!(
                    "rollback: remove_connectivity_group {} failed: {}",
                    cg_id, e,
                );
            }
        }
    }));
    Ok(cg_id)
}

fn store_share_config(
    store: &storage::ShareStateStore,
    api_key: &str,
    rollback: &mut RollbackVec,
) -> Result<()> {
    let cfg = storage::ShareConfig::new(&api_key);
    store.save_share_config(&cfg)?;
    rollback.push(Box::pin({
        let store = store.clone();
        async move {
            if let Err(e) = store.delete() {
                eprintln!("rollback: deleting store failed: {}", e);
            }
        }
    }));
    Ok(())
}

async fn create_wispers_node(
    store: &storage::ShareStateStore,
    rollback: &mut RollbackVec,
) -> Result<wispers_connect::Node> {
    let node_storage = wispers_connect::NodeStorage::new(store.clone());
    let node = node_storage.restore_or_init_node().await?;
    rollback.push(Box::pin(async move {
        if let Err(e) = node_storage.delete_state() {
            eprintln!("rollback: deleting node state failed: {}", e);
        }
    }));
    Ok(node)
}

//-- serve ----------------------------------------------------------------------

fn serve(share: &str, local_port: &u16) -> Result<()> {
    println!("serve({}, {});", share, local_port);
    // What this needs to do:
    // - Read the config of the given share (api key and cgID), which we'll use
    //   for invites
    // - Init the node (must be registered or activated)
    // - Serve in the foreground (this part is mostly what wconnect did, except
    //   we need to handle the requests, so we can inject or elide headers)

    // Load config and node state from storage.
    let store = storage::ShareStateStore::new(share)?;
    let Some(share_config) = store.load_share_config()? else {
        anyhow::bail!("Unknown share {}", share);
    };
    let node_storage = wispers_connect::NodeStorage::new(store);
    let Some(registration) = node_storage.read_registration()? else {
        anyhow::bail!("Wispers node for share {} has not been registered", share);
    };
    Ok(())
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

fn status() -> Result<()> {
    println!("status();");
    // What this needs to do:
    // - List all shares (using the file system)
    // - For each, check if there's a server running
    // - If so, query the server for info/stats to display
    // - Print it all nicely
    Ok(())
}

fn logs(follow: &bool, share: &str) -> Result<()> {
    println!("logs({}, {});", follow, share);
    // - Find the latest logs of the given share (platform dependent)
    // - Print them, or if -f is given, tail them
    Ok(())
}

fn is_valid_share_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64  // pick your limit
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        && !(s.starts_with('-') || s.starts_with('_'))
        && !(s.ends_with('-') || s.ends_with('_'))
}
