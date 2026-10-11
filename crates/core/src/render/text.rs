use std::sync::OnceLock;

use ab_glyph::{
    point, Font, FontArc, FontRef, FontVec, OutlineCurve, OutlinedGlyph, PxScale, ScaleFont,
};
use tiny_skia::{
    Color, FillRule, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

/// Bundled UI font (RobotoMono, SIL Open Font License — see assets/font-OFL.txt).
const FONT_BYTES: &[u8] = include_bytes!("../../assets/font.ttf");

/// A font to measure and draw text with: the bundled UI font, or one loaded
/// at runtime (e.g. a CJK font for map labels).
#[derive(Clone)]
pub struct TextFont(FontArc);

impl std::fmt::Debug for TextFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextFont")
    }
}

impl TextFont {
    /// The bundled UI font (Latin only).
    pub fn bundled() -> &'static Self {
        static FONT: OnceLock<TextFont> = OnceLock::new();
        FONT.get_or_init(|| {
            Self(FontArc::new(
                FontRef::try_from_slice(FONT_BYTES).expect("bundled font must be valid"),
            ))
        })
    }

    /// Load a TrueType/OpenType font, or face `index` of a collection
    /// (`.ttc`). `None` if the data isn't a usable font.
    pub fn from_bytes(data: Vec<u8>, index: u32) -> Option<Self> {
        FontVec::try_from_vec_and_index(data, index)
            .ok()
            .map(|f| Self(FontArc::new(f)))
    }

    /// Whether the font has a glyph for every non-whitespace character.
    pub fn covers(&self, text: &str) -> bool {
        text.chars()
            .all(|c| c.is_whitespace() || self.0.glyph_id(c).0 != 0)
    }

    /// Total advance width of `text` at the given pixel size.
    pub fn measure(&self, text: &str, size: f32) -> f32 {
        let scaled = self.0.as_scaled(PxScale::from(size));
        text.chars()
            .map(|c| scaled.h_advance(self.0.glyph_id(c)))
            .sum()
    }

    /// Advance width of each character of `text` at the given pixel size.
    pub fn advances(&self, text: &str, size: f32) -> Vec<f32> {
        let scaled = self.0.as_scaled(PxScale::from(size));
        text.chars()
            .map(|c| scaled.h_advance(self.0.glyph_id(c)))
            .collect()
    }

    /// Draw `text` one glyph at a time, each centered on a `(x, y, angle)`
    /// from `glyphs` (radians, clockwise from the x axis) and turned to
    /// it, with a `halo` stroke `halo_width` pixels wide under the fill.
    /// Used for labels that follow a line.
    pub fn draw_turned(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        size: f32,
        glyphs: &[(f32, f32, f32)],
        color: Color,
        halo: Option<(Color, f32)>,
    ) {
        let font = &self.0;
        let scaled = font.as_scaled(PxScale::from(size));
        let factor = scaled.scale_factor();
        // Baseline below the glyph's center so the text sits centered on it.
        let rise = self.rise(size);
        let shapes: Vec<(tiny_skia::Path, Transform)> = text
            .chars()
            .zip(glyphs)
            .filter_map(|(ch, &(x, y, angle))| {
                let id = font.glyph_id(ch);
                let path = glyph_path(&font.outline(id)?)?;
                let (sin, cos) = angle.sin_cos();
                // Glyph origin: half an advance back along the line, `rise`
                // below it, from the glyph's center.
                let (ox, oy) = (-scaled.h_advance(id) / 2.0, rise);
                let origin = (x + ox * cos - oy * sin, y + ox * sin + oy * cos);
                let transform = Transform::from_row(
                    factor.horizontal * cos,
                    factor.horizontal * sin,
                    -factor.vertical * sin,
                    factor.vertical * cos,
                    origin.0,
                    origin.1,
                );
                Some((path, transform))
            })
            .collect();
        let mut paint = Paint {
            anti_alias: true,
            ..Default::default()
        };
        // All halos first, so one glyph's halo never covers its neighbour.
        if let Some((halo, width)) = halo {
            paint.set_color(halo);
            let stroke = Stroke {
                width: width * 2.0 / factor.horizontal.max(f32::EPSILON),
                line_join: LineJoin::Round,
                ..Default::default()
            };
            for (path, transform) in &shapes {
                pixmap.stroke_path(path, &paint, &stroke, *transform, None);
            }
        }
        paint.set_color(color);
        for (path, transform) in &shapes {
            pixmap.fill_path(path, &paint, FillRule::Winding, *transform, None);
        }
    }

    /// The CSS `font-size` that draws text as large as this font does at
    /// pixel size `size` (which sets the ascent-to-descent height, not the
    /// em square).
    pub fn em_size(&self, size: f32) -> f32 {
        let height = self.0.height_unscaled();
        match self.0.units_per_em() {
            Some(em) if height > 0.0 => size * em / height,
            _ => size,
        }
    }

    /// Distance from the top of a line to its baseline at pixel size `size`,
    /// as [`TextFont::draw`] lays it out.
    pub fn ascent(&self, size: f32) -> f32 {
        self.0.as_scaled(PxScale::from(size)).ascent()
    }

    /// How far below a glyph's center its baseline sits at pixel size
    /// `size`, as [`TextFont::draw_turned`] places it.
    pub fn rise(&self, size: f32) -> f32 {
        let scaled = self.0.as_scaled(PxScale::from(size));
        (scaled.ascent() + scaled.descent()) / 2.0
    }

    /// The line height (ascent + descent + line gap) at the given pixel size.
    pub fn line_height(&self, size: f32) -> f32 {
        let scaled = self.0.as_scaled(PxScale::from(size));
        scaled.height() + scaled.line_gap()
    }

    /// Draw a single line of text with its top-left at `(x, y)`, returning
    /// the advance width. Glyph coverage is alpha-blended onto the canvas.
    pub fn draw(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        x: f32,
        y: f32,
        size: f32,
        color: Color,
    ) -> f32 {
        self.draw_layers(pixmap, text, size, &[(x, y, color)])
    }

    /// Draw the same line of text once per `(x, y, color)` layer, in order
    /// (e.g. a halo of offset copies, then the text on top), returning the
    /// advance width. Same pixels as calling [`TextFont::draw`] per layer, but
    /// each glyph's outline is read from the font only once.
    pub fn draw_layers(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        size: f32,
        layers: &[(f32, f32, Color)],
    ) -> f32 {
        let font = &self.0;
        let scaled = font.as_scaled(PxScale::from(size));
        let glyphs: Vec<_> = text
            .chars()
            .map(|ch| {
                let id = font.glyph_id(ch);
                (id, font.outline(id), scaled.h_advance(id))
            })
            .collect();
        let scale_factor = scaled.scale_factor();
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let pixels = pixmap.pixels_mut();

        let mut advance = 0.0;
        for &(x, y, color) in layers {
            let baseline = y + scaled.ascent();
            let mut caret = x;
            for (id, outline, h_advance) in &glyphs {
                if let Some(outline) = outline {
                    let glyph = id.with_scale_and_position(size, point(caret, baseline));
                    let outlined = OutlinedGlyph::new(glyph, outline.clone(), scale_factor);
                    let bounds = outlined.px_bounds();
                    outlined.draw(|gx, gy, coverage| {
                        let px = bounds.min.x as i32 + gx as i32;
                        let py = bounds.min.y as i32 + gy as i32;
                        if px < 0 || py < 0 || px >= width || py >= height {
                            return;
                        }
                        let idx = (py * width + px) as usize;
                        pixels[idx] = blend(pixels[idx], color, coverage);
                    });
                }
                caret += h_advance;
            }
            advance = caret - x;
        }
        advance
    }
}

/// A glyph outline (font units, y up) as a path with y flipped down.
fn glyph_path(outline: &ab_glyph::Outline) -> Option<tiny_skia::Path> {
    let mut builder = PathBuilder::new();
    let p = |q: ab_glyph::Point| (q.x, -q.y);
    let mut last = None;
    for curve in &outline.curves {
        let start = match curve {
            OutlineCurve::Line(a, _) | OutlineCurve::Quad(a, _, _) => *a,
            OutlineCurve::Cubic(a, _, _, _) => *a,
        };
        if last != Some(start) {
            if last.is_some() {
                builder.close();
            }
            let (sx, sy) = p(start);
            builder.move_to(sx, sy);
        }
        last = Some(match curve {
            OutlineCurve::Line(_, b) => {
                let (bx, by) = p(*b);
                builder.line_to(bx, by);
                *b
            }
            OutlineCurve::Quad(_, c, b) => {
                let ((cx, cy), (bx, by)) = (p(*c), p(*b));
                builder.quad_to(cx, cy, bx, by);
                *b
            }
            OutlineCurve::Cubic(_, c1, c2, b) => {
                let ((ax, ay), (cx, cy), (bx, by)) = (p(*c1), p(*c2), p(*b));
                builder.cubic_to(ax, ay, cx, cy, bx, by);
                *b
            }
        });
    }
    builder.close();
    builder.finish()
}

/// Total advance width of `text` at the given pixel size (bundled font).
pub fn measure(text: &str, size: f32) -> f32 {
    TextFont::bundled().measure(text, size)
}

/// The line height (ascent + descent + line gap) at the given pixel size
/// (bundled font).
pub fn line_height(size: f32) -> f32 {
    TextFont::bundled().line_height(size)
}

/// Draw a single line of text with its top-left at `(x, y)` in the bundled
/// font, returning the advance width.
pub fn draw_text(pixmap: &mut Pixmap, text: &str, x: f32, y: f32, size: f32, color: Color) -> f32 {
    TextFont::bundled().draw(pixmap, text, x, y, size, color)
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
    fn layers_draw_the_same_pixels_as_separate_calls() {
        let font = TextFont::bundled();
        let layers = [
            (10.3, 12.7, Color::BLACK),
            (11.6, 13.1, Color::from_rgba8(200, 30, 30, 160)),
            (11.0, 12.9, Color::WHITE),
        ];
        let mut one = Pixmap::new(160, 48).unwrap();
        let mut many = one.clone();
        let w = font.draw_layers(&mut one, "Halo 42", 20.0, &layers);
        for &(x, y, c) in &layers {
            assert!((font.draw(&mut many, "Halo 42", x, y, 20.0, c) - w).abs() < 1e-3);
        }
        assert_eq!(one.data(), many.data());
    }

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
    fn coverage_detects_missing_glyphs() {
        assert!(TextFont::bundled().covers("Seoul 12"));
        assert!(!TextFont::bundled().covers("서울"));
        assert!(TextFont::from_bytes(vec![1, 2, 3], 0).is_none());
    }

    #[test]
    fn offscreen_text_is_safe() {
        let mut pixmap = Pixmap::new(40, 20).unwrap();
        pixmap.fill(Color::BLACK);
        draw_text(&mut pixmap, "overflow", -100.0, -100.0, 16.0, Color::WHITE);
        draw_text(&mut pixmap, "overflow", 200.0, 200.0, 16.0, Color::WHITE);
    }
}
