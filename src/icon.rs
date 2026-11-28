// SPDX-License-Identifier: GPL-3.0-or-later
//! The window icon, drawn in code from the same shapes as
//! `assets/stowlock.svg` (so no image libraries are needed).

const BG: [u8; 3] = [0x0C, 0x10, 0x16];
/// The edge: the accent at 35 % over the background (House Standard v1.3).
const EDGE: [u8; 3] = [0x1A, 0x54, 0x44];
/// The lock's body: a deeper emerald, so the whole symbol is in StowLock's
/// accent (House Standard v1.3) and the lid still reads apart from it.
const BODY: [u8; 3] = [0x05, 0x96, 0x69];
const EMERALD: [u8; 3] = [0x34, 0xD3, 0x99];

fn in_rrect(x: f32, y: f32, rx: f32, ry: f32, w: f32, h: f32, r: f32) -> bool {
    if x < rx || x > rx + w || y < ry || y > ry + h {
        return false;
    }
    let cx = x.clamp(rx + r, rx + w - r);
    let cy = y.clamp(ry + r, ry + h - r);
    (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn in_rect(x: f32, y: f32, rx: f32, ry: f32, w: f32, h: f32) -> bool {
    x >= rx && x <= rx + w && y >= ry && y <= ry + h
}

/// The keyhole: bold enough to still read at 16 px.
fn in_keyhole(x: f32, y: f32) -> bool {
    if (x - 128.0).powi(2) + (y - 150.0).powi(2) <= 24.0f32.powi(2) {
        return true;
    }
    // Stem 114,158 → 142,158 → 150,196 → 106,196.
    if (158.0..=196.0).contains(&y) {
        let t = (y - 158.0) / 38.0;
        return x >= 114.0 - 8.0 * t && x <= 142.0 + 8.0 * t;
    }
    false
}

/// Colour at a point in the 256×256 design (same shapes as
/// `assets/stowlock.svg`), or None for transparent.
fn sample(x: f32, y: f32) -> Option<[u8; 3]> {
    if !in_rrect(x, y, 0.0, 0.0, 256.0, 256.0, 56.0) {
        return None;
    }
    // A thin edge (1 px at 64 px and up; it stays visible when small).
    if !in_rrect(x, y, 4.0, 4.0, 248.0, 248.0, 52.0) {
        return Some(EDGE);
    }
    if in_rrect(x, y, 40.0, 44.0, 176.0, 172.0, 28.0) {
        let dark = in_rect(x, y, 40.0, 96.0, 176.0, 12.0) || in_keyhole(x, y);
        if dark {
            return Some(BG);
        }
        return Some(if y < 96.0 { EMERALD } else { BODY });
    }
    Some(BG)
}

/// RGBA pixels at the given square size.
pub fn rgba(size: u32) -> Vec<u8> {
    let mut out = vec![0u8; (size * size * 4) as usize];
    let scale = 256.0 / size as f32;
    const SS: u32 = 4;
    for py in 0..size {
        for px in 0..size {
            let (mut r, mut g, mut b, mut a) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (px as f32 + (sx as f32 + 0.5) / SS as f32) * scale;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) * scale;
                    if let Some(c) = sample(x, y) {
                        r += c[0] as u32;
                        g += c[1] as u32;
                        b += c[2] as u32;
                        a += 255;
                    }
                }
            }
            let n = SS * SS;
            let covered = a / 255;
            let i = ((py * size + px) * 4) as usize;
            if let (Some(rr), Some(gg), Some(bb)) = (
                r.checked_div(covered),
                g.checked_div(covered),
                b.checked_div(covered),
            ) {
                out[i] = rr as u8;
                out[i + 1] = gg as u8;
                out[i + 2] = bb as u8;
            }
            out[i + 3] = (a / n) as u8;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draws_expected_colours() {
        let px = rgba(256);
        let at = |x: usize, y: usize| &px[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4];
        assert_eq!(&at(128, 70)[..3], &EMERALD, "lid");
        assert_eq!(&at(70, 180)[..3], &BODY, "body");
        assert_eq!(&at(128, 150)[..3], &BG, "keyhole");
        assert_eq!(at(1, 1)[3], 0, "rounded corner is transparent");
    }

    /// Writes previews at 16, 32 and 128 px when STOWLOCK_ICON_PREVIEW names
    /// a folder (raw RGBA; used to check the icon still reads when small).
    #[test]
    fn previews_when_asked() {
        if let Ok(dir) = std::env::var("STOWLOCK_ICON_PREVIEW") {
            for size in [16, 32, 128] {
                std::fs::write(format!("{dir}/icon-{size}.rgba"), rgba(size)).unwrap();
            }
        }
    }
}
