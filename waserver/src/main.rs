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
    Ok(())
}

fn serve(share: &str, local_port: &u16) -> Result<()> {
    println!("serve({}, {});", share, local_port);
    Ok(())
}

fn start(share: &str, local_port: &u16) -> Result<()> {
    println!("start({}, {});", share, local_port);
    Ok(())
}

fn stop(share: &str) -> Result<()> {
    println!("stop({});", share);
    Ok(())
}

fn status() -> Result<()> {
    println!("status();");
    Ok(())
}

fn logs(follow: &bool, share: &str) -> Result<()> {
    println!("logs({}, {});", follow, share);
    Ok(())
}
