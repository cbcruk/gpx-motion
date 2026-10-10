use tiny_skia::{Color, Pixmap};

use super::text::{draw_text, line_height, measure};

/// Corner to anchor an overlay against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Anchor {
    fn is_right(self) -> bool {
        matches!(self, Anchor::TopRight | Anchor::BottomRight)
    }

    fn is_bottom(self) -> bool {
        matches!(self, Anchor::BottomLeft | Anchor::BottomRight)
    }
}

/// Visual parameters for overlay text.
#[derive(Debug, Clone, Copy)]
pub struct OverlayStyle {
    pub size: f32,
    pub color: Color,
    pub margin: f32,
    /// Extra space kept clear above the bottom edge (e.g. for a chart panel).
    pub bottom_inset: f32,
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            size: 18.0,
            color: Color::WHITE,
            margin: 12.0,
            bottom_inset: 0.0,
        }
    }
}

impl OverlayStyle {
    /// The same style for a canvas with `ratio` device pixels per CSS pixel.
    pub fn scaled(mut self, ratio: f32) -> Self {
        self.size *= ratio;
        self.margin *= ratio;
        self.bottom_inset *= ratio;
        self
    }
}

/// Draw a block of text lines anchored to a corner of the canvas. A 1px dark
/// shadow is drawn under each line for legibility over busy map tiles.
pub fn draw_block(pixmap: &mut Pixmap, lines: &[String], anchor: Anchor, style: &OverlayStyle) {
    if lines.is_empty() {
        return;
    }
    let lh = line_height(style.size);
    let block_h = lh * lines.len() as f32;
    let canvas_w = pixmap.width() as f32;
    let canvas_h = pixmap.height() as f32;

    let top = if anchor.is_bottom() {
        canvas_h - style.margin - style.bottom_inset - block_h
    } else {
        style.margin
    };

    let shadow = Color::from_rgba8(0, 0, 0, 0xc0);
    for (i, line) in lines.iter().enumerate() {
        let y = top + i as f32 * lh;
        let x = if anchor.is_right() {
            canvas_w - style.margin - measure(line, style.size)
        } else {
            style.margin
        };
        draw_text(pixmap, line, x + 1.0, y + 1.0, style.size, shadow);
        draw_text(pixmap, line, x, y, style.size, style.color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filled() -> Pixmap {
        let mut p = Pixmap::new(320, 200).unwrap();
        p.fill(Color::from_rgba8(0x40, 0x40, 0x40, 0xff));
        p
    }

    #[test]
    fn block_draws_text() {
        let mut p = filled();
        let before = p.data().to_vec();
        draw_block(
            &mut p,
            &["lat 37.5".to_string(), "lon 127.0".to_string()],
            Anchor::BottomLeft,
            &OverlayStyle::default(),
        );
        assert_ne!(before, p.data());
    }

    #[test]
    fn empty_block_is_noop() {
        let mut p = filled();
        let before = p.data().to_vec();
        draw_block(&mut p, &[], Anchor::TopRight, &OverlayStyle::default());
        assert_eq!(before, p.data());
    }

    #[test]
    fn right_anchor_writes_on_right_half() {
        let mut left = filled();
        let mut right = filled();
        let style = OverlayStyle::default();
        draw_block(&mut left, &["TESTING".to_string()], Anchor::TopLeft, &style);
        draw_block(
            &mut right,
            &["TESTING".to_string()],
            Anchor::TopRight,
            &style,
        );
        // The right-anchored text should not touch the far-left margin column.
        let col_changed = |p: &Pixmap, x: u32| -> bool {
            (0..p.height()).any(|y| p.pixel(x, y).unwrap() != filled().pixel(x, y).unwrap())
        };
        assert!(col_changed(&left, 14));
        assert!(!col_changed(&right, 14));
    }
}
