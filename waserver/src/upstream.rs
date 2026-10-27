//! Connections from the host node to what it serves: the upstreams of
//! reverse-proxy apps, the origins of forward-proxy apps, and the probes of
//! `waserver status`.

use tokio::net::TcpStream;

/// Connect to `addr` (`host:port`), dialing all its addresses at once and
/// keeping the first that answers.
///
/// This works around slowness with `TcpStream::connect` and IPv4-only
/// upstreams on Windows. There, `localhost` resolves to `::1` first, causing a
/// 2s timeout before timing out and trying 127.0.0.1.
pub async fn connect(addr: &str) -> std::io::Result<TcpStream> {
    let mut attempts = tokio::task::JoinSet::new();
    for addr in tokio::net::lookup_host(addr).await? {
        attempts.spawn(TcpStream::connect(addr));
    }
    let mut last_err = None;
    while let Some(attempt) = attempts.join_next().await {
        match attempt {
            // Dropping the set aborts the attempts still in flight.
            Ok(Ok(tcp)) => return Ok(tcp),
            Ok(Err(e)) => last_err = Some(e),
            Err(e) => last_err = Some(std::io::Error::other(e)),
        }
    }
    Err(last_err.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::NotFound, "no addresses resolved")
    }))
}
