//! Draws the tray's progress ring. Done by hand into an RGBA buffer so the
//! tray needs no image or graphics dependency.

use std::f32::consts::TAU;

/// Side length in pixels. Twice the 22 pt macOS menu bar height so it stays
/// sharp on Retina; other platforms scale it down.
pub const SIZE: u32 = 44;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// The unfilled part of the ring. Mid grey at partial opacity reads on both
/// light and dark bars.
const TRACK: Rgb = Rgb(150, 150, 150);
const TRACK_ALPHA: f32 = 0.45;

/// RGBA pixels of a ring filled clockwise from 12 o'clock.
pub fn render(fraction: f32, fill: Rgb) -> Vec<u8> {
    let size = SIZE as usize;
    let centre = SIZE as f32 / 2.0;
    let outer = centre - 1.0;
    let inner = outer * 0.64;
    let fraction = fraction.clamp(0.0, 1.0);

    let mut rgba = vec![0_u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            let dx = x as f32 + 0.5 - centre;
            let dy = y as f32 + 0.5 - centre;
            let distance = dx.hypot(dy);

            // 1 inside the ring, fading to 0 across one pixel at each edge.
            let coverage = (outer - distance + 0.5)
                .clamp(0.0, 1.0)
                .min((distance - inner + 0.5).clamp(0.0, 1.0));
            if coverage <= 0.0 {
                continue;
            }

            // Clockwise turn from 12 o'clock, 0..1.
            let turn = dx.atan2(-dy).rem_euclid(TAU) / TAU;
            let (Rgb(r, g, b), alpha) = if turn < fraction {
                (fill, 1.0)
            } else {
                (TRACK, TRACK_ALPHA)
            };

            let at = (y * size + x) * 4;
            rgba[at..at + 4].copy_from_slice(&[r, g, b, (alpha * coverage * 255.0).round() as u8]);
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILL: Rgb = Rgb(10, 20, 30);

    fn pixel(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
        let at = (y * SIZE as usize + x) * 4;
        rgba[at..at + 4].try_into().unwrap()
    }

    #[test]
    fn buffer_matches_declared_size() {
        assert_eq!(render(0.5, FILL).len(), (SIZE * SIZE * 4) as usize);
    }

    #[test]
    fn centre_and_corners_are_transparent() {
        let rgba = render(1.0, FILL);
        let mid = SIZE as usize / 2;
        assert_eq!(pixel(&rgba, mid, mid)[3], 0);
        assert_eq!(pixel(&rgba, 0, 0)[3], 0);
    }

    #[test]
    fn fills_clockwise_from_twelve_oclock() {
        // Just right of 12 o'clock (turn ~0.005) and just above 9 o'clock
        // (turn ~0.75), both mid-ring.
        let (top, left) = ((22, 4), (4, 22));
        let rgb = |p: [u8; 4]| [p[0], p[1], p[2]];

        let half = render(0.5, FILL);
        assert_eq!(rgb(pixel(&half, top.0, top.1)), [10, 20, 30]);
        assert_eq!(rgb(pixel(&half, left.0, left.1)), [150, 150, 150]);

        let ninety = render(0.9, FILL);
        assert_eq!(rgb(pixel(&ninety, left.0, left.1)), [10, 20, 30]);
    }

    #[test]
    fn empty_ring_is_all_track_and_full_ring_all_fill() {
        let is_fill = |p: [u8; 4]| p[..3] == [10, 20, 30];
        let visible = |rgba: &[u8]| {
            let (pixels, _) = rgba.as_chunks::<4>();
            pixels
                .iter()
                .filter(|p| p[3] > 0)
                .copied()
                .collect::<Vec<_>>()
        };
        assert!(visible(&render(0.0, FILL)).iter().all(|p| !is_fill(*p)));
        assert!(visible(&render(1.0, FILL)).iter().all(|p| is_fill(*p)));
    }

    #[test]
    fn out_of_range_fractions_are_clamped() {
        assert_eq!(render(-3.0, FILL), render(0.0, FILL));
        assert_eq!(render(7.0, FILL), render(1.0, FILL));
    }
}
