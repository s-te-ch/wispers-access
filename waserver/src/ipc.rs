//! Inter-process communication between server and cli tool.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use tokio::io::BufReader;

#[cfg(windows)]
use tokio::net::{TcpListener, TcpStream};
#[cfg(unix)]
use tokio::net::{UnixListener, UnixStream};

#[cfg(unix)]
pub type IpcStream = UnixStream;
#[cfg(windows)]
pub type IpcStream = TcpStream;
#[cfg(unix)]
type ReadHalf = tokio::net::unix::OwnedReadHalf;
#[cfg(windows)]
type ReadHalf = tokio::net::tcp::OwnedReadHalf;
#[cfg(unix)]
type WriteHalf = tokio::net::unix::OwnedWriteHalf;
#[cfg(windows)]
type WriteHalf = tokio::net::tcp::OwnedWriteHalf;

#[cfg(unix)]
pub struct Server {
    listener: UnixListener,
}

#[cfg(unix)]
impl Server {
    pub async fn bind(share: &str) -> Result<Self> {
        let path = ipc_path(share);

        // Check for a stale socket.
        if path.exists() {
            match UnixStream::connect(&path).await {
                Ok(_) => {
                    anyhow::bail!("Server already running at {:?}", path);
                }
                Err(_) => {
                    // Stale socket, remove it
                    fs::remove_file(&path).context("failed to remove stale socket")?;
                }
            }
        }

        // Ensure the socket directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Bind the socket.
        let listener = UnixListener::bind(&path).context("failed to bind socket")?;

        Ok(Self { listener })
    }

    pub async fn accept(&self) -> Result<IpcStream> {
        let (stream, _addr) = self.listener.accept().await?;
        Ok(stream)
    }
}

#[cfg(windows)]
pub struct Server {
    listener: TcpListener,
    /// Password that Windows clients must send before any request. Stored in
    /// the `.port` file alongside the port, readable only by the user.
    windows_ipc_password: String,
}

#[cfg(windows)]
impl Server {
    pub async fn bind(share: &str) -> Result<Self> {
        use rand::Rng;

        let path = ipc_path(share);

        // Check for a stale socket.
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path)
                && let Some((port, _)) = parse_port_file(&contents)
                && TcpStream::connect(("127.0.0.1", port)).await.is_ok()
            {
                anyhow::bail!("daemon already running on port {}", port);
            }
            fs::remove_file(&path)
                .await
                .context("failed to remove stale port file")?;
        }

        // Ensure the ports directory exists.
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Bind the local port.
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .context("failed to bind TCP listener")?;
        let port = listener.local_addr()?.port();

        // Write a random password for IPC auth.
        let password: String = rand::rng()
            .sample_iter(rand::distr::Alphanumeric)
            .take(32)
            .map(char::from)
            .collect();
        fs::write(&path, format!("{}:{}", port, password)).context("failed to write port file")?;

        Ok(Self {
            listener,
            windows_ipc_password: password,
        })
    }

    pub async fn accept(&self) -> Result<IpcStream> {
        loop {
            let (stream, _addr) = self.listener.accept().await?;
            let mut buf_stream = BufReader::new(stream);
            let mut password_line = String::new();
            match buf_stream.read_line(&mut password_line).await {
                Ok(0) => continue,
                Ok(_) if password_line.trim() == self.windows_ipc_password => {
                    return Ok(buf_stream.into_inner());
                }
                _ => continue,
            }
        }
    }
}

/// Client for connecting to the daemon.
pub struct Client {
    reader: BufReader<ReadHalf>,
    writer: WriteHalf,
}

impl Client {
    #[cfg(unix)]
    pub async fn connect(share: &str) -> Result<Self> {
        let path = ipc_path(share);
        let stream = UnixStream::connect(&path).await.with_context(|| {
            format!("failed to connect to server at {:?} (is it running?)", path)
        })?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }

    #[cfg(windows)]
    pub async fn connect(share: &str) -> Result<Self> {
        let path = ipc_path(share);
        let contents = fs::read_to_string(&path)
            .await
            .with_context(|| format!("daemon not running (no port file {:?})", path))?;
        let (port, password) = parse_port_file(&contents).context("invalid daemon port file")?;
        let stream = TcpStream::connect(("127.0.0.1", port))
            .await
            .with_context(|| format!("daemon not running (port {})", port))?;
        let (reader, mut writer) = stream.into_split();
        // Send IPC password
        writer.write_all(password.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
        })
    }
}

#[cfg(unix)]
fn ipc_path(share: &str) -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join(".waserver").join("sockets");
    dir.join(format!("{}.sock", share))
}

#[cfg(windows)]
fn ipc_path(share: &str) -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join(".waserver").join("ports");
    return dir.join(format!("{}.port", share));
}

#[cfg(windows)]
fn parse_port_file(contents: &str) -> Option<(u16, &str)> {
    let contents = contents.trim();
    let colon = contents.find(':')?;
    let port: u16 = contents[..colon].parse().ok()?;
    let password = &contents[colon + 1..];
    Some((port, password))
}
