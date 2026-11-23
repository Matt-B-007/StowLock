// SPDX-License-Identifier: GPL-3.0-or-later
//! The window icon, drawn in code from the same shapes as
//! `assets/stowlock.svg` (so no image libraries are needed).

const BG: [u8; 3] = [0x18, 0x1C, 0x24];
const SKY: [u8; 3] = [0x38, 0xBD, 0xF8];
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

fn in_keyhole(x: f32, y: f32) -> bool {
    if (x - 128.0).powi(2) + (y - 169.0).powi(2) <= 15.0f32.powi(2) {
        return true;
    }
    // Trapezoid 121,174 → 135,174 → 140,194 → 116,194, grown by ~2.
    if (172.0..=196.0).contains(&y) {
        let t = ((y - 174.0) / 20.0).clamp(0.0, 1.0);
        let left = 121.0 - 5.0 * t - 2.0;
        let right = 135.0 + 5.0 * t + 2.0;
        return x >= left && x <= right;
    }
    false
}

/// Colour at a point in the 256×256 design, or None for transparent.
fn sample(x: f32, y: f32) -> Option<[u8; 3]> {
    if !in_rrect(x, y, 0.0, 0.0, 256.0, 256.0, 60.0) {
        return None;
    }
    let teeth = [
        (116.0, 119.0),
        (128.0, 127.0),
        (116.0, 135.0),
        (128.0, 143.0),
    ];
    let dark = in_rect(x, y, 52.0, 104.0, 152.0, 7.0)
        || teeth
            .iter()
            .any(|(tx, ty)| in_rect(x, y, *tx, *ty, 12.0, 8.0))
        || in_keyhole(x, y);
    if in_rrect(x, y, 52.0, 58.0, 152.0, 148.0, 24.0) {
        if dark {
            return Some(BG);
        }
        return Some(if y < 104.0 { EMERALD } else { SKY });
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
        assert_eq!(&at(128, 80)[..3], &EMERALD, "lid");
        assert_eq!(&at(70, 180)[..3], &SKY, "body");
        assert_eq!(&at(128, 169)[..3], &BG, "keyhole");
        assert_eq!(at(1, 1)[3], 0, "rounded corner is transparent");
    }
}
