//! Programmatic brand icon — the same green disc used for the tray icon,
//! window icon, exe icon (.ico via examples/make-icon) and installer.

/// RGBA pixel data of the OcyDash disc icon rendered at `size`×`size`.
/// Outer ring #5AC88C + dark core — matches the headless tray icon.
pub fn disc_rgba(size: u32) -> Vec<u8> {
    let mut rgba = vec![0u8; (size * size * 4) as usize];
    let half = size as f64 / 2.0;
    let r_out = half * 0.92;
    let r_in = half * 0.55;
    for y in 0..size {
        for x in 0..size {
            let dx = x as f64 + 0.5 - half;
            let dy = y as f64 + 0.5 - half;
            let d = (dx * dx + dy * dy).sqrt();
            let px = &mut rgba[(y * size + x) as usize * 4..][..4];
            // 1px anti-aliased edge on each circle
            let (r, g, b, a) = if d <= r_in {
                (22, 26, 34, 255)
            } else if d <= r_out {
                (90, 200, 140, 255)
            } else {
                (0, 0, 0, 0)
            };
            px.copy_from_slice(&[r, g, b, a]);
        }
    }
    rgba
}
