use tiny_skia::{Color, Pixmap};

use super::placer::PlacedLabel;
use super::style::LabelStyle;

/// Where [`draw_labels`] moves covered glyphs: far enough off any canvas.
const OFF_CANVAS_PX: f32 = 1e6;

/// Draw placed labels with a halo so they read over any map.
pub fn draw_labels(pixmap: &mut Pixmap, labels: &[PlacedLabel], style: &LabelStyle) {
    for label in labels {
        let fade = |c: Color| crate::color::scale_alpha(c, label.opacity);
        let (color, halo) = (fade(style.color), fade(style.halo));
        let r = halo_radius(label.size);
        if !label.glyphs.is_empty() {
            // Covered glyphs go far off the canvas, which clips them away.
            let glyphs: Vec<(f32, f32, f32)> = label
                .glyphs
                .iter()
                .enumerate()
                .map(|(i, &g)| match label.covered.get(i) {
                    Some(true) => (-OFF_CANVAS_PX, -OFF_CANVAS_PX, g.2),
                    _ => g,
                })
                .collect();
            let (text, size) = (&label.text, label.size);
            style
                .font
                .draw_turned(pixmap, text, size, &glyphs, color, Some((halo, r)));
            continue;
        }
        let mut layers: Vec<(f32, f32, Color)> = [
            (-1.0, -1.0),
            (0.0, -1.0),
            (1.0, -1.0),
            (-1.0, 0.0),
            (1.0, 0.0),
            (-1.0, 1.0),
            (0.0, 1.0),
            (1.0, 1.0),
        ]
        .iter()
        .map(|(dx, dy)| (label.x + dx * r, label.y + dy * r, halo))
        .collect();
        layers.push((label.x, label.y, color));
        style
            .font
            .draw_layers(pixmap, &label.text, label.size, &layers);
    }
}

/// Width in pixels of the halo around a label drawn at pixel size `size`.
pub fn halo_radius(size: f32) -> f32 {
    (size / 10.0).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vector::labels::test_support::*;

    #[test]
    fn covered_glyphs_are_left_out() {
        let draw = |covered: Vec<bool>| {
            let mut pixmap = Pixmap::new(120, 40).unwrap();
            pixmap.fill(Color::BLACK);
            let label = PlacedLabel {
                text: "HH".into(),
                x: 20.0,
                y: 10.0,
                size: 20.0,
                opacity: 1.0,
                glyphs: vec![(30.0, 20.0, 0.0), (90.0, 20.0, 0.0)],
                covered,
            };
            draw_labels(&mut pixmap, &[label], &style());
            let lit = |x0: u32, x1: u32| {
                (x0..x1)
                    .flat_map(|x| (0..40).map(move |y| (x, y)))
                    .filter(|&(x, y)| pixmap.pixel(x, y).unwrap().red() > 128)
                    .count()
            };
            (lit(0, 60), lit(60, 120))
        };
        let (left, right) = draw(Vec::new());
        assert!(left > 0 && right > 0);
        assert_eq!(draw(vec![true, false]), (0, right));
    }

    #[test]
    fn draws_labels() {
        let mut pixmap = Pixmap::new(200, 60).unwrap();
        pixmap.fill(Color::from_rgba8(0x40, 0x40, 0x40, 0xff));
        let before = pixmap.data().to_vec();
        let label = PlacedLabel {
            text: "Seoul".into(),
            x: 10.0,
            y: 10.0,
            size: 20.0,
            opacity: 1.0,
            glyphs: Vec::new(),
            covered: Vec::new(),
        };
        draw_labels(&mut pixmap, &[label], &style());
        assert_ne!(before, pixmap.data());
    }
}
