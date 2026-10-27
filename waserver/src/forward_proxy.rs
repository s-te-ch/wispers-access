//! Forward-proxy mode implementation.

use crate::config::ShareConfig;
use crate::upstream;
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tracing::{info, warn};
use wire::{ForwardProxyError, ForwardProxyPreamble, ForwardProxyReply};
use wispers_access_wire as wire;

/// Timeout for `CONNECT` requests. Without a limit, the guest node's browser
/// would wait for the OS's TCP timeout, which can be a minute or more.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Serve one FORWARD_PROXY stream, after `protocol::handle` has already read
/// the type byte. Parse the preamble, then establish the tunnel.
pub async fn serve<S>(mut stream: S, config: &ShareConfig, user_id: String) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let preamble: ForwardProxyPreamble = wire::read_message(&mut stream)
        .await
        .context("reading FORWARD_PROXY preamble")?;
    let target = format!("{}:{}", preamble.host, preamble.port);
    if !config.has_proxied_origin(&preamble.host, preamble.port) {
        warn!(%user_id, %target, "tunnel to an origin the share does not proxy");
        return refuse(stream, ForwardProxyError::NotAllowed).await;
    }
    let mut tcp = match connect(&target).await {
        Ok(tcp) => tcp,
        Err(e) => {
            warn!(%user_id, %target, error = %e, "tunnel target unreachable");
            return refuse(stream, ForwardProxyError::Unreachable).await;
        }
    };
    wire::write_message(&mut stream, &ForwardProxyReply::default())
        .await
        .context("writing FORWARD_PROXY reply")?;

    info!(%user_id, %target, "tunnel opened");
    let opened = Instant::now();
    // A FIN on either side is passed on while the other direction keeps
    // flowing, so this returns once both have closed.
    match tokio::io::copy_bidirectional(&mut stream, &mut tcp).await {
        Ok((guest_to_target, target_to_guest)) => info!(
            %user_id,
            %target,
            guest_to_target,
            target_to_guest,
            duration = ?opened.elapsed(),
            "tunnel closed"
        ),
        // Usually a reset from either end, such as a closed browser tab.
        // copy_bidirectional loses the byte counts then.
        Err(e) => info!(
            %user_id,
            %target,
            error = %e,
            duration = ?opened.elapsed(),
            "tunnel aborted"
        ),
    }
    Ok(())
}

async fn connect(target: &str) -> std::io::Result<tokio::net::TcpStream> {
    match tokio::time::timeout(CONNECT_TIMEOUT, upstream::connect(target)).await {
        Ok(connected) => connected,
        Err(_) => Err(std::io::ErrorKind::TimedOut.into()),
    }
}

/// Answer with `error` and finish the stream.
async fn refuse<S: AsyncWrite + Unpin>(mut stream: S, error: ForwardProxyError) -> Result<()> {
    let reply = ForwardProxyReply { error: Some(error) };
    wire::write_message(&mut stream, &reply)
        .await
        .context("writing FORWARD_PROXY reply")?;
    stream.shutdown().await.ok();
    Ok(())
}
