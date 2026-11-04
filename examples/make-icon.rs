//! Generates assets/ocydash.ico — a multi-size PNG-compressed ICO of the
//! OcyDash green disc (same artwork as the runtime tray icon).
//! Run: `cargo run --example make-icon`

use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder, RgbaImage};

fn disc(size: u32) -> RgbaImage {
    let mut img = RgbaImage::new(size, size);
    let half = size as f64 / 2.0;
    let r_out = half * 0.92;
    let r_in = half * 0.55;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 + 0.5 - half;
            let dy = y as f64 + 0.5 - half;
            let d = (dx * dx + dy * dy).sqrt();
            let px = img.get_pixel_mut(x, y);
            *px = if d <= r_in {
                image::Rgba([22, 26, 34, 255])
            } else if d <= r_out {
                image::Rgba([90, 200, 140, 255])
            } else {
                image::Rgba([0, 0, 0, 0])
            };
        }
    }
    img
}

fn main() {
    std::fs::create_dir_all("assets").unwrap();
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];

    // PNG-encode each size (PNG-compressed ICO frames are valid since Vista)
    let mut frames: Vec<(u32, Vec<u8>)> = Vec::new();
    for &s in &sizes {
        let img = disc(s);
        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(img.as_raw(), s, s, ExtendedColorType::Rgba8)
            .unwrap();
        frames.push((s, png));
    }

    // Hand-roll the ICO container: ICONDIR + ICONDIRENTRY[] + PNG payloads
    let mut ico: Vec<u8> = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]); // reserved=0, type=1 (icon)
    ico.extend_from_slice(&(frames.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * frames.len();
    for (s, png) in &frames {
        let dim = if *s >= 256 { 0u8 } else { *s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]); // w, h, palette, reserved
        ico.extend_from_slice(&1u16.to_le_bytes()); // color planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bpp
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(off as u32).to_le_bytes());
        off += png.len();
    }
    for (_, png) in &frames {
        ico.extend_from_slice(png);
    }
    std::fs::write("assets/ocydash.ico", &ico).unwrap();
    println!("wrote assets/ocydash.ico ({} bytes)", ico.len());
}
