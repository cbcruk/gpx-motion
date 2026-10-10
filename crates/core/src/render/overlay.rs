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

/// Text size the shadow offset of one pixel is tuned for.
const DEFAULT_SIZE: f32 = 18.0;

impl Default for OverlayStyle {
    fn default() -> Self {
        Self {
            size: DEFAULT_SIZE,
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

/// The canvas area a [`draw_block`] call with these lines covers (for
/// keeping map labels clear of it), or `None` for no lines.
pub fn block_rect(
    canvas: (f32, f32),
    lines: &[String],
    anchor: Anchor,
    style: &OverlayStyle,
) -> Option<tiny_skia::Rect> {
    if lines.is_empty() {
        return None;
    }
    let block_h = line_height(style.size) * lines.len() as f32;
    let width = lines
        .iter()
        .map(|l| measure(l, style.size))
        .fold(0.0, f32::max);
    let top = if anchor.is_bottom() {
        canvas.1 - style.margin - style.bottom_inset - block_h
    } else {
        style.margin
    };
    let left = if anchor.is_right() {
        canvas.0 - style.margin - width
    } else {
        style.margin
    };
    tiny_skia::Rect::from_xywh(left, top, width, block_h)
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
        let offset = style.size / DEFAULT_SIZE;
        draw_text(pixmap, line, x + offset, y + offset, style.size, shadow);
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
    fn block_rect_matches_the_anchor() {
        let lines = vec!["abc".to_string(), "defgh".to_string()];
        let style = OverlayStyle::default();
        let r = block_rect((320.0, 200.0), &lines, Anchor::BottomRight, &style).unwrap();
        assert!((r.right() - (320.0 - style.margin)).abs() < 1e-3);
        assert!((r.bottom() - (200.0 - style.margin)).abs() < 1e-3);
        assert!(block_rect((320.0, 200.0), &[], Anchor::TopLeft, &style).is_none());
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
