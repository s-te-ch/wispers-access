//! HTTP forwarding with (optional) header rewriting.

use anyhow::Result;
use wispers_connect;

pub async fn handle_quic_stream(
    stream: wispers_connect::QuicStream,
    local_port: u16,
) -> Result<()> {
    println!("Handling HTTP request on stream {} for port {}", stream.id(), local_port);
    // TODO
    Ok(())
}
