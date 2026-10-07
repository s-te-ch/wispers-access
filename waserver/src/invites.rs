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

/// How to draw QR codes in the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum QrStyle {
    /// Small, default, but doesn't work in all terminals. Two module rows per
    /// line, using half-block glyphs. Ragged in terminals whose font draws
    /// those glyphs badly, such as macOS Terminal.
    Small,
    /// Twice as large, compatible with most terminals, but may not fit in small
    /// terminal windows. One painted cell per module, no glyphs.
    Compat,
    /// No QR code shown.
    Off,
}

/// Render the code. With `json`, prints `{"code": …}`. Otherwise prints a QR
/// code in the terminal in the given style, then the code as text. Also writes
/// a PNG at `png` if given.
pub fn render_code(code: &str, json: bool, qr: QrStyle, png: Option<&Path>) -> Result<()> {
    if json {
        println!("{}", serde_json::json!({ "code": code }));
    } else {
        let rendered = match qr {
            QrStyle::Small => Some(render_qr_half_blocks(&qr_code(code)?)),
            QrStyle::Compat => {
                let qr = qr_code(code)?;
                Some(render_qr_cells(&qr, cells_per_module(&qr)))
            }
            QrStyle::Off => None,
        };
        if let Some(rendered) = rendered {
            println!("{rendered}");
        }
        println!("Invite code (valid for 24 hours):\n\n  {}\n", code);
        match qr {
            QrStyle::Small => {
                println!("Weird QR code? Try `--qr=compat`, or `--png PATH` for an image file.")
            }
            QrStyle::Compat if png.is_none() => {
                println!("`--png PATH` writes the QR code to an image file.")
            }
            _ => {}
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

/// Palette indices of #000000 and #ffffff. We explicitly draw black on white,
/// so dark theme terminals don't break the QR code. We use the 256-colour
/// palette (instead of truecolour) because a terminal without 24-bit support
/// drops truecolour codes and renders every line as one solid bar.
const BLACK: &str = "16";
const WHITE: &str = "231";

/// Render a QR code in "small" format,  with two module rows per line.
fn render_qr_half_blocks(qr: &qrcode::QrCode) -> String {
    let grid = Grid::new(qr);
    let mut out = String::new();
    let mut y = 0;
    while y < grid.size {
        out.push_str(&format!("\x1b[38;5;{BLACK}m"));
        let mut bg = "";
        for x in 0..grid.size {
            let (cell_bg, glyph) = match (grid.dark(x, y), grid.dark(x, y + 1)) {
                (true, true) => (BLACK, ' '),
                (false, false) => (WHITE, ' '),
                (true, false) => (WHITE, '\u{2580}'),
                (false, true) => (WHITE, '\u{2584}'),
            };
            if cell_bg != bg {
                out.push_str(&format!("\x1b[48;5;{cell_bg}m"));
                bg = cell_bg;
            }
            out.push(glyph);
        }
        out.push_str("\x1b[0m\n"); // reset colours at end of each line
        y += 2;
    }
    out
}

/// Render a QR code in "compat" format. Every module is `cells` character cells
/// painted by their background colour, with spaces on top. No glyph is
/// involved, so no font can get it wrong.
fn render_qr_cells(qr: &qrcode::QrCode, cells: usize) -> String {
    let grid = Grid::new(qr);
    let mut out = String::new();
    for y in 0..grid.size {
        let mut bg = "";
        for x in 0..grid.size {
            let cell_bg = if grid.dark(x, y) { BLACK } else { WHITE };
            if cell_bg != bg {
                out.push_str(&format!("\x1b[48;5;{cell_bg}m"));
                bg = cell_bg;
            }
            out.push_str(&" ".repeat(cells));
        }
        out.push_str("\x1b[0m\n"); // reset colours at end of each line
    }
    out
}

/// Two cells per module where the terminal is wide enough for the code and
/// its quiet zone, so the modules come out square. One otherwise (also when
/// stdout is not a terminal).
fn cells_per_module(qr: &qrcode::QrCode) -> usize {
    let width = qr.width() + 2 * QUIET;
    match terminal_size::terminal_size() {
        Some((terminal_size::Width(columns), _)) if usize::from(columns) >= 2 * width => 2,
        _ => 1,
    }
}

/// The light border around a rendered code, in modules.
const QUIET: usize = 4;

/// A code's modules with the quiet zone around them, input for rendering.
struct Grid {
    /// Modules per side, quiet zone included.
    size: usize,
    width: usize,
    modules: Vec<qrcode::Color>,
}

impl Grid {
    fn new(qr: &qrcode::QrCode) -> Self {
        Self {
            size: qr.width() + 2 * QUIET,
            width: qr.width(),
            modules: qr.to_colors(),
        }
    }

    /// Dark module at (col x, row y)? The quiet zone is light, and so is
    /// anything beyond the grid.
    fn dark(&self, x: usize, y: usize) -> bool {
        if x < QUIET || y < QUIET || x >= QUIET + self.width || y >= QUIET + self.width {
            return false;
        }
        matches!(
            self.modules[(y - QUIET) * self.width + (x - QUIET)],
            qrcode::Color::Dark
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads each rendering back into modules and compares them with the code
    /// it came from, cell by cell.
    #[test]
    fn terminal_renderings_round_trip() {
        let qr = qr_code("wax1_iroh_ab12cd_0123456789abcdef").unwrap();
        let grid = Grid::new(&qr);

        let half = render_qr_half_blocks(&qr);
        let lines: Vec<&str> = half.lines().collect();
        assert_eq!(lines.len(), grid.size.div_ceil(2));
        for (row, line) in lines.iter().enumerate() {
            let y = row * 2;
            let mut x = 0;
            each_cell(line, |bg_black, glyph| {
                let (top, bottom) = match glyph {
                    ' ' => (bg_black, bg_black),
                    '\u{2580}' => (true, false),
                    '\u{2584}' => (false, true),
                    other => panic!("unexpected glyph {other:?}"),
                };
                assert!(
                    !bg_black || glyph == ' ',
                    "half blocks are drawn on white only"
                );
                assert_eq!(top, grid.dark(x, y), "top module at ({x}, {y})");
                assert_eq!(
                    bottom,
                    grid.dark(x, y + 1),
                    "bottom module at ({x}, {})",
                    y + 1
                );
                x += 1;
            });
            assert_eq!(x, grid.size, "line {row} has {x} cells");
        }

        for cells in [1, 2] {
            let full = render_qr_cells(&qr, cells);
            let lines: Vec<&str> = full.lines().collect();
            assert_eq!(lines.len(), grid.size);
            for (y, line) in lines.iter().enumerate() {
                let mut n = 0;
                each_cell(line, |bg_black, glyph| {
                    assert_eq!(glyph, ' ');
                    assert_eq!(
                        bg_black,
                        grid.dark(n / cells, y),
                        "module at ({}, {y})",
                        n / cells
                    );
                    n += 1;
                });
                assert_eq!(n, grid.size * cells, "line {y} has {n} cells");
            }
        }
    }

    /// Walks a rendered line, calling `f` with the background (black?) and
    /// the glyph of every cell. Only the escapes the renderers emit are
    /// accepted.
    fn each_cell(line: &str, mut f: impl FnMut(bool, char)) {
        let mut bg_black = false;
        let mut rest = line;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("\x1b[") {
                let end = after.find('m').unwrap();
                match &after[..end] {
                    "38;5;16" | "0" => {}
                    "48;5;16" => bg_black = true,
                    "48;5;231" => bg_black = false,
                    other => panic!("unexpected escape {other}"),
                }
                rest = &after[end + 1..];
                continue;
            }
            let glyph = rest.chars().next().unwrap();
            rest = &rest[glyph.len_utf8()..];
            f(bg_black, glyph);
        }
    }
}
