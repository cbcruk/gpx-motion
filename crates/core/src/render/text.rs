use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use tiny_skia::{Color, Pixmap, PremultipliedColorU8};

/// Bundled UI font (RobotoMono, SIL Open Font License — see assets/font-OFL.txt).
const FONT_BYTES: &[u8] = include_bytes!("../../assets/font.ttf");

fn font() -> FontRef<'static> {
    FontRef::try_from_slice(FONT_BYTES).expect("bundled font must be valid")
}

/// Total advance width of `text` at the given pixel size.
pub fn measure(text: &str, size: f32) -> f32 {
    let font = font();
    let scaled = font.as_scaled(PxScale::from(size));
    text.chars()
        .map(|c| scaled.h_advance(font.glyph_id(c)))
        .sum()
}

/// The line height (ascent + descent + line gap) at the given pixel size.
pub fn line_height(size: f32) -> f32 {
    let font = font();
    let scaled = font.as_scaled(PxScale::from(size));
    scaled.height() + scaled.line_gap()
}

/// Draw a single line of text with its top-left at `(x, y)`, returning the
/// advance width. Glyph coverage is alpha-blended onto the canvas.
pub fn draw_text(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: Color) -> f32 {
    let font = font();
    let scaled = font.as_scaled(PxScale::from(size));
    let baseline = y + scaled.ascent();
    let width = pixmap.width() as i32;
    let height = pixmap.height() as i32;
    let pixels = pixmap.pixels_mut();

    let mut caret = x;
    for ch in text.chars() {
        let id = font.glyph_id(ch);
        let glyph = id.with_scale_and_position(size, point(caret, baseline));
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= width || py >= height {
                    return;
                }
                let idx = (py * width + px) as usize;
                pixels[idx] = blend(pixels[idx], color, coverage);
            });
        }
        caret += scaled.h_advance(id);
    }
    caret - x
}

/// Alpha-blend a straight `color` at `coverage` over a premultiplied pixel.
fn blend(dst: PremultipliedColorU8, color: Color, coverage: f32) -> PremultipliedColorU8 {
    let sa = color.alpha() * coverage;
    if sa <= 0.0 {
        return dst;
    }
    let inv = 1.0 - sa;
    let blend_ch = |src: f32, dst: u8| ((src * 255.0 * sa) + dst as f32 * inv).round() as u8;

    let r = blend_ch(color.red(), dst.red());
    let g = blend_ch(color.green(), dst.green());
    let b = blend_ch(color.blue(), dst.blue());
    let a = (sa * 255.0 + dst.alpha() as f32 * inv).round() as u8;
    PremultipliedColorU8::from_rgba(r.min(a), g.min(a), b.min(a), a).unwrap_or(dst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_scales_with_size() {
        let small = measure("ABC", 12.0);
        let big = measure("ABC", 24.0);
        assert!(big > small * 1.9 && big < small * 2.1);
    }

    #[test]
    fn monospace_width_is_proportional_to_length() {
        let one = measure("0", 16.0);
        let four = measure("0000", 16.0);
        assert!((four - one * 4.0).abs() < 0.5);
    }

    #[test]
    fn draw_text_marks_pixels() {
        let mut pixmap = Pixmap::new(120, 40).unwrap();
        pixmap.fill(Color::BLACK);
        let before = pixmap.data().to_vec();
        draw_text(&mut pixmap, "12.3 km/h", 4.0, 4.0, 20.0, Color::WHITE);
        assert_ne!(before, pixmap.data(), "text should modify pixels");
    }

    #[test]
    fn offscreen_text_is_safe() {
        let mut pixmap = Pixmap::new(40, 20).unwrap();
        pixmap.fill(Color::BLACK);
        draw_text(&mut pixmap, "overflow", -100.0, -100.0, 16.0, Color::WHITE);
        draw_text(&mut pixmap, "overflow", 200.0, 200.0, 16.0, Color::WHITE);
    }
}
