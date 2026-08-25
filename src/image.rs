//! Inline image protocols: Sixel (DCS q ... ST) and the Kitty graphics
//! protocol (APC _G ... \). v1 covers the common case for both — enough to
//! render `chafa`/`img2sixel` output and simple Kitty `icat` transfers —
//! not the full spec (no animation frames, no PNG payloads for Kitty yet).
//!
//! Decoded images become an RGBA buffer + cell-space placement, which the
//! renderer uploads as a GPU texture and draws as a textured quad over the
//! grid, same idea as text glyphs but one quad instead of one per cell.

use anyhow::{bail, Result};

pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// Cell column/row where the image's top-left should be anchored.
    pub cell_col: usize,
    pub cell_row: usize,
}

/// Decode a Sixel data stream (the bytes between the DCS introducer and the
/// ST terminator, `\x1bP...q ... \x1b\\`, NOT including those framing
/// bytes — the PTY reader strips them before calling this).
pub fn decode_sixel(data: &[u8], cell_col: usize, cell_row: usize) -> Result<DecodedImage> {
    // Sixel: 6 vertical pixels packed per data byte (bits 0-5, value 0-63,
    // encoded as byte+63 in the stream). '#' introduces/selects a color
    // register, '!' is a repeat-count, '$' = carriage return (back to col
    // 0, same sixel row), '-' = next sixel row (down 6px).
    let mut palette: Vec<(u8, u8, u8)> = vec![(0, 0, 0); 256];
    let mut x: i64 = 0;
    let mut y: i64 = 0;
    let mut cur_color = 0usize;
    let mut max_x: i64 = 0;
    let mut max_y: i64 = 0;
    let mut pixels: Vec<(i64, i64, usize)> = Vec::new(); // sparse; converted to RGBA at the end
    let mut repeat = 1u32;
    let mut i = 0;

    while i < data.len() {
        let b = data[i];
        match b {
            b'#' => {
                i += 1;
                let (num, len) = read_int(&data[i..]);
                i += len;
                cur_color = num.unwrap_or(0) as usize;
                // Optional `;Pu;Px;Py;Pz` color definition (HLS/RGB) — parse if present.
                if i < data.len() && data[i] == b';' {
                    let mut parts = [0i64; 4];
                    let mut pi = 0;
                    while i < data.len() && data[i] == b';' && pi < 4 {
                        i += 1;
                        let (n, len) = read_int(&data[i..]);
                        parts[pi] = n.unwrap_or(0);
                        i += len;
                        pi += 1;
                    }
                    // parts = [colorspace(2=RGB,1=HLS), p1, p2, p3] as percentages 0-100 for RGB
                    if parts[0] == 2 {
                        let scale = |v: i64| ((v.clamp(0, 100) as f32) * 255.0 / 100.0) as u8;
                        let color = (scale(parts[1]), scale(parts[2]), scale(parts[3]));
                        if cur_color < palette.len() {
                            palette[cur_color] = color;
                        }
                    }
                }
                continue;
            }
            b'!' => {
                i += 1;
                let (num, len) = read_int(&data[i..]);
                repeat = num.unwrap_or(1).max(1) as u32;
                i += len;
                continue;
            }
            b'$' => {
                x = 0;
                i += 1;
                continue;
            }
            b'-' => {
                x = 0;
                y += 6;
                i += 1;
                continue;
            }
            0x3f..=0x7e => {
                let value = b - 0x3f; // 6-bit sixel value
                for _ in 0..repeat {
                    for bit in 0..6 {
                        if value & (1 << bit) != 0 {
                            let py = y + bit as i64;
                            pixels.push((x, py, cur_color));
                            max_x = max_x.max(x);
                            max_y = max_y.max(py);
                        }
                    }
                    x += 1;
                }
                repeat = 1;
                i += 1;
                continue;
            }
            _ => {
                i += 1;
            }
        }
    }

    if pixels.is_empty() {
        bail!("sixel stream kosong / tidak valid");
    }

    let w = (max_x + 1).max(1) as u32;
    let h = (max_y + 1).max(1) as u32;
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    for (px, py, color_idx) in pixels {
        if px < 0 || py < 0 {
            continue;
        }
        let (r, g, b) = palette.get(color_idx).copied().unwrap_or((255, 255, 255));
        let idx = ((py as u32 * w + px as u32) * 4) as usize;
        if idx + 3 < rgba.len() {
            rgba[idx] = r;
            rgba[idx + 1] = g;
            rgba[idx + 2] = b;
            rgba[idx + 3] = 255;
        }
    }

    Ok(DecodedImage {
        width: w,
        height: h,
        rgba,
        cell_col,
        cell_row,
    })
}

fn read_int(data: &[u8]) -> (Option<i64>, usize) {
    let mut i = 0;
    let mut val: i64 = 0;
    let mut any = false;
    while i < data.len() && data[i].is_ascii_digit() {
        val = val * 10 + (data[i] - b'0') as i64;
        any = true;
        i += 1;
    }
    (if any { Some(val) } else { None }, i)
}

/// Kitty graphics protocol: parses the `key=value,key=value...` control
/// data from an APC `_G...` payload plus the base64 pixel payload after the
/// `;`. Supports the common direct-transmission case: f=32 (RGBA) or f=24
/// (RGB) raw pixels, a=T (transmit+display). f=100 (PNG) is TODO — would
/// need a PNG decoder; raw RGB/RGBA covers `icat --format`-style tools.
pub fn decode_kitty(control: &str, payload_b64: &str, cell_col: usize, cell_row: usize) -> Result<DecodedImage> {
    let mut format = 32u32;
    let mut width = 0u32;
    let mut height = 0u32;

    for kv in control.split(',') {
        let mut it = kv.splitn(2, '=');
        let (Some(k), Some(v)) = (it.next(), it.next()) else { continue };
        match k {
            "f" => format = v.parse().unwrap_or(32),
            "s" => width = v.parse().unwrap_or(0),
            "v" => height = v.parse().unwrap_or(0),
            _ => {}
        }
    }

    if format == 100 {
        bail!("Kitty format=100 (PNG) belum didukung di v1, pakai f=32/f=24 raw");
    }

    let raw = base64_decode(payload_b64)?;
    let channels = if format == 24 { 3 } else { 4 };
    if width == 0 || height == 0 {
        // Some senders omit s/v when it's implied by a prior transmit; bail
        // rather than guess a wrong aspect ratio.
        bail!("Kitty payload tanpa dimensi (s=/v=) tidak didukung di v1");
    }
    let expected = (width * height) as usize * channels;
    if raw.len() < expected {
        bail!("Kitty payload lebih pendek dari yang diharapkan");
    }

    let mut rgba = vec![0u8; (width * height * 4) as usize];
    if channels == 4 {
        rgba.copy_from_slice(&raw[..expected]);
    } else {
        for px in 0..(width * height) as usize {
            rgba[px * 4] = raw[px * 3];
            rgba[px * 4 + 1] = raw[px * 3 + 1];
            rgba[px * 4 + 2] = raw[px * 3 + 2];
            rgba[px * 4 + 3] = 255;
        }
    }

    Ok(DecodedImage {
        width,
        height,
        rgba,
        cell_col,
        cell_row,
    })
}

/// Minimal base64 decoder so we don't pull in another crate just for this.
fn base64_decode(s: &str) -> Result<Vec<u8>> {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut lut = [255u8; 256];
    for (i, &c) in TABLE.iter().enumerate() {
        lut[c as usize] = i as u8;
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| *b != b'\n' && *b != b'\r').collect();
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    let mut chunk = [0u8; 4];
    let mut chunk_len = 0;
    for &b in &bytes {
        if b == b'=' {
            break;
        }
        let v = lut[b as usize];
        if v == 255 {
            continue;
        }
        chunk[chunk_len] = v;
        chunk_len += 1;
        if chunk_len == 4 {
            out.push((chunk[0] << 2) | (chunk[1] >> 4));
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
            out.push((chunk[2] << 6) | chunk[3]);
            chunk_len = 0;
        }
    }
    if chunk_len >= 2 {
        out.push((chunk[0] << 2) | (chunk[1] >> 4));
        if chunk_len == 3 {
            out.push((chunk[1] << 4) | (chunk[2] >> 2));
        }
    }
    Ok(out)
}
