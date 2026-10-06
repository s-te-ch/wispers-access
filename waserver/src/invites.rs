//! `waserver invite`: asks the share's running server for an invite code and
//! shows it as text and as a QR code, or as JSON for scripts.

use crate::ipc;
use anyhow::{Context, Result};
use std::path::Path;

/// Asks the share's running server to mint an invite.
pub async fn request_code(share: &str, node_name: &str, user_id: &str) -> Result<String> {
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
        }) => Ok(invite.code),
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
}

/// Render the code. With `json`, prints `{"code": …}`. Otherwise prints the
/// code as human readable text, plus a QR code in the terminal if `qr` is true.
/// Also writes a PNG at `png` if given.
pub fn render_code(code: &str, json: bool, qr: bool, png: Option<&Path>) -> Result<()> {
    if json {
        println!("{}", serde_json::json!({ "code": code }));
    } else {
        println!("Invite code (valid for 24 hours):\n\n  {}\n", code);
        if qr {
            println!("{}", render_qr_ansi(&qr_code(code)?));
        }
    }
    if let Some(path) = png {
        write_png(code, path)?;
        if !json {
            println!("QR code written to {}", path.display());
        }
    }
    Ok(())
}

fn write_png(code: &str, path: &Path) -> Result<()> {
    let img = qr_code(code)?
        .render::<image::Luma<u8>>()
        .min_dimensions(360, 360)
        .build();
    img.save(path)
        .with_context(|| format!("cannot write {}", path.display()))
}

fn qr_code(code: &str) -> Result<qrcode::QrCode> {
    qrcode::QrCode::new(code.as_bytes()).context("cannot build QR code")
}

/// Render a QR code to a terminal string that scans regardless of terminal
/// theme.
///
/// `qrcode`'s `unicode::Dense1x2` renderer draws modules in the terminal's
/// *foreground* colour on its *background*, so on a dark terminal the QR comes
/// out inverted (light modules on dark) and scanners — which expect
/// dark-on-light — reject it. Here every module gets an explicit black/white,
/// so it is always dark-on-light. The colours come from the 256-colour palette,
/// not truecolour because a terminal without 24-bit support drops the
/// truecolour codes and renders every line as one solid bar in its default
/// colours, while the palette works everywhere. `▀` (upper half block) packs
/// two module rows per line: the glyph's foreground is the top module, its
/// background the bottom one. (`--png` stays the colour-independent fallback
/// for terminals that strip ANSI.)
fn render_qr_ansi(qr: &qrcode::QrCode) -> String {
    const QUIET: usize = 4; // standard quiet zone, in modules
    const BLACK: &str = "16"; // palette index of #000000
    const WHITE: &str = "231"; // palette index of #ffffff

    let w = qr.width();
    let modules = qr.to_colors();
    let size = w + 2 * QUIET;
    // Dark module at (col x, row y)? The quiet-zone border is light.
    let dark = |x: usize, y: usize| -> bool {
        if x < QUIET || y < QUIET || x >= QUIET + w || y >= QUIET + w {
            return false;
        }
        matches!(modules[(y - QUIET) * w + (x - QUIET)], qrcode::Color::Dark)
    };

    let mut out = String::new();
    let mut y = 0;
    while y < size {
        for x in 0..size {
            let fg = if dark(x, y) { BLACK } else { WHITE };
            let bg = if y + 1 < size && dark(x, y + 1) {
                BLACK
            } else {
                WHITE
            };
            out.push_str(&format!("\x1b[38;5;{fg}m\x1b[48;5;{bg}m\u{2580}"));
        }
        out.push_str("\x1b[0m\n"); // reset colours at end of each line
        y += 2;
    }
    out
}
