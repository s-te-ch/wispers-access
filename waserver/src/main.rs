use anyhow::Result;
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
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init { share, api_key } => init(&api_key, &share),
        Command::Serve { share, local_port } => serve(&share, &local_port),
        Command::Start { share, local_port } => start(&share, &local_port),
        Command::Stop { share } => stop(&share),
        Command::Status => status(),
        Command::Logs { follow, share } => logs(&follow, &share),
    }
}

fn init(api_key: &str, share: &str) -> Result<()> {
    println!("init({}, {});", api_key, share);
    // What this needs to do:
    // - Create a connectivity group using the Rest API (name TBD, something
    //   like "Wispers Access - $name"?)
    // - Store this as one file in the config dir (key and cgID)
    // - Get a registration code for node 1, name "waserver"
    // - Create a Node, with storage implementation using another file in the
    //   config dir
    // - Register the node (this writes automatically)
    Ok(())
}

fn serve(share: &str, local_port: &u16) -> Result<()> {
    println!("serve({}, {});", share, local_port);
    // What this needs to do:
    // - Read the config of the given share (api key and cgID), which we'll use
    //   for invites
    // - Init the node (must be registered or activated)
    // - Serve in the foreground (this part is mostly what wconnect did, except
    //   we need to handle the requests, so we can inject or elide headers)
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
