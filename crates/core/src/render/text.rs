//! Text layout and drawing with a bundled or runtime-loaded font, shaping
//! complex scripts when the `shaping` feature is on.

#[cfg(feature = "shaping")]
use std::sync::Arc;
use std::sync::OnceLock;

use ab_glyph::{
    point, Font, FontArc, FontRef, FontVec, GlyphId, OutlineCurve, OutlinedGlyph, PxScale,
    ScaleFont,
};
use tiny_skia::{
    Color, FillRule, LineJoin, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Stroke, Transform,
};

/// Bundled UI font (RobotoMono, SIL Open Font License — see assets/font-OFL.txt).
const FONT_BYTES: &[u8] = include_bytes!("../../assets/font.ttf");

/// A font to measure and draw text with: the bundled UI font, or one loaded
/// at runtime (e.g. a CJK font for map labels).
///
/// Scripts whose letters change shape or order with their neighbours
/// (Arabic, Hebrew, the Indic scripts, …) are shaped with rustybuzz (the
/// `shaping` feature); other text is laid out one character after another.
#[derive(Clone)]
pub struct TextFont {
    font: FontArc,
    #[cfg(feature = "shaping")]
    shaper: Shaper,
}

/// One glyph of laid-out text: its advance, and its offset from the pen
/// (y down), in pixels or (as shaped and cached) in font units.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Placed {
    id: GlyphId,
    advance: f32,
    dx: f32,
    dy: f32,
}

/// The font file for rustybuzz, and the texts shaped with it so far (in
/// font units, so any size reuses them).
#[cfg(feature = "shaping")]
#[derive(Clone)]
struct Shaper {
    data: Arc<[u8]>,
    index: u32,
    shaped: Arc<std::sync::Mutex<std::collections::HashMap<String, Arc<[Placed]>>>>,
}

#[cfg(feature = "shaping")]
impl Shaper {
    fn new(data: Arc<[u8]>, index: u32) -> Self {
        Self {
            data,
            index,
            shaped: Arc::default(),
        }
    }

    /// `text` shaped in font units, in visual order (left to right on
    /// screen), or `None` if the font data can't be shaped.
    ///
    /// The text is split into runs of one direction first (the Unicode
    /// bidirectional algorithm), so digits and Latin words inside a
    /// right-to-left name keep their own order.
    fn shape(&self, text: &str) -> Option<Arc<[Placed]>> {
        if let Some(hit) = self.shaped.lock().ok()?.get(text) {
            return Some(Arc::clone(hit));
        }
        let face = rustybuzz::Face::from_slice(&self.data, self.index)?;
        let bidi = unicode_bidi::BidiInfo::new(text, None);
        let mut glyphs = Vec::new();
        for paragraph in &bidi.paragraphs {
            let (levels, runs) = bidi.visual_runs(paragraph, paragraph.range.clone());
            for run in runs {
                let mut buffer = rustybuzz::UnicodeBuffer::new();
                buffer.push_str(&text[run.clone()]);
                buffer.set_direction(if levels[run.start].is_rtl() {
                    rustybuzz::Direction::RightToLeft
                } else {
                    rustybuzz::Direction::LeftToRight
                });
                buffer.guess_segment_properties();
                let shaped = rustybuzz::shape(&face, &[], buffer);
                glyphs.extend(
                    shaped
                        .glyph_infos()
                        .iter()
                        .zip(shaped.glyph_positions())
                        .map(|(info, pos)| Placed {
                            id: GlyphId(info.glyph_id as u16),
                            advance: pos.x_advance as f32,
                            dx: pos.x_offset as f32,
                            dy: -pos.y_offset as f32,
                        }),
                );
            }
        }
        let glyphs: Arc<[Placed]> = glyphs.into();
        self.shaped
            .lock()
            .ok()?
            .insert(text.to_string(), Arc::clone(&glyphs));
        Some(glyphs)
    }
}

impl std::fmt::Debug for TextFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TextFont")
    }
}

impl TextFont {
    /// The bundled UI font (Latin only).
    pub(crate) fn bundled() -> &'static Self {
        static FONT: OnceLock<TextFont> = OnceLock::new();
        FONT.get_or_init(|| Self {
            font: FontArc::new(
                FontRef::try_from_slice(FONT_BYTES).expect("bundled font must be valid"),
            ),
            #[cfg(feature = "shaping")]
            shaper: Shaper::new(Arc::from(FONT_BYTES), 0),
        })
    }

    /// Load a TrueType/OpenType font, or face `index` of a collection
    /// (`.ttc`). `None` if the data isn't a usable font. With the `shaping`
    /// feature the font file is kept twice (once for rustybuzz).
    pub fn from_bytes(data: Vec<u8>, index: u32) -> Option<Self> {
        #[cfg(feature = "shaping")]
        let shaper = Shaper::new(Arc::from(data.as_slice()), index);
        FontVec::try_from_vec_and_index(data, index)
            .ok()
            .map(|f| Self {
                font: FontArc::new(f),
                #[cfg(feature = "shaping")]
                shaper,
            })
    }

    /// `text`'s glyphs at pixel size `size`, left to right on screen: shaped
    /// when the script needs it (and the `shaping` feature is on), else one
    /// per character.
    fn layout(&self, text: &str, size: f32) -> Vec<Placed> {
        #[cfg(feature = "shaping")]
        if needs_shaping(text) {
            let height = self.font.height_unscaled();
            if let Some(shaped) = self.shaper.shape(text).filter(|_| height > 0.0) {
                let scale = size / height;
                return shaped
                    .iter()
                    .map(|g| Placed {
                        id: g.id,
                        advance: g.advance * scale,
                        dx: g.dx * scale,
                        dy: g.dy * scale,
                    })
                    .collect();
            }
        }
        let scaled = self.font.as_scaled(PxScale::from(size));
        text.chars()
            .map(|c| {
                let id = self.font.glyph_id(c);
                Placed {
                    id,
                    advance: scaled.h_advance(id),
                    dx: 0.0,
                    dy: 0.0,
                }
            })
            .collect()
    }

    /// Whether `text` comes out right in this font: it has every glyph, and
    /// the script is one that doesn't need shaping or shaping is built in.
    pub fn draws(&self, text: &str) -> bool {
        self.covers(text) && (cfg!(feature = "shaping") || !needs_shaping(text))
    }

    /// Whether the font has a glyph for every non-whitespace character.
    pub fn covers(&self, text: &str) -> bool {
        text.chars()
            .all(|c| c.is_whitespace() || self.font.glyph_id(c).0 != 0)
    }

    /// Total advance width of `text` at the given pixel size.
    pub(crate) fn measure(&self, text: &str, size: f32) -> f32 {
        self.layout(text, size).iter().map(|g| g.advance).sum()
    }

    /// Advance width of each glyph of `text` at the given pixel size, left
    /// to right on screen: one per character unless the script is shaped
    /// (see [`TextFont`]), where letters may merge, split, or reorder.
    pub(crate) fn advances(&self, text: &str, size: f32) -> Vec<f32> {
        self.layout(text, size).iter().map(|g| g.advance).collect()
    }

    /// Draw `text` one glyph at a time, each centered on a `(x, y, angle)`
    /// from `glyphs` (radians, clockwise from the x axis; one per glyph of
    /// [`TextFont::advances`]) and turned to it, with a `halo` stroke
    /// `halo_width` pixels wide under the fill. Used for labels that follow
    /// a line.
    pub(crate) fn draw_turned(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        size: f32,
        glyphs: &[(f32, f32, f32)],
        color: Color,
        halo: Option<(Color, f32)>,
    ) {
        let font = &self.font;
        let scaled = font.as_scaled(PxScale::from(size));
        let factor = scaled.scale_factor();
        // Baseline below the glyph's center so the text sits centered on it.
        let rise = self.rise(size);
        let shapes: Vec<(tiny_skia::Path, Transform)> = self
            .layout(text, size)
            .iter()
            .zip(glyphs)
            .filter_map(|(glyph, &(x, y, angle))| {
                let path = glyph_path(&font.outline(glyph.id)?)?;
                let (sin, cos) = angle.sin_cos();
                // Glyph origin: half an advance back along the line, `rise`
                // below it, from the glyph's center, plus the shaping offset.
                let (ox, oy) = (-glyph.advance / 2.0 + glyph.dx, rise + glyph.dy);
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
        let height = self.font.height_unscaled();
        match self.font.units_per_em() {
            Some(em) if height > 0.0 => size * em / height,
            _ => size,
        }
    }

    /// Distance from the top of a line to its baseline at pixel size `size`,
    /// as [`TextFont::draw`] lays it out.
    pub fn ascent(&self, size: f32) -> f32 {
        self.font.as_scaled(PxScale::from(size)).ascent()
    }

    /// How far below a glyph's center its baseline sits at pixel size
    /// `size`, as glyphs turned along a line are placed.
    pub fn rise(&self, size: f32) -> f32 {
        let scaled = self.font.as_scaled(PxScale::from(size));
        (scaled.ascent() + scaled.descent()) / 2.0
    }

    /// The line height (ascent + descent + line gap) at the given pixel size.
    pub(crate) fn line_height(&self, size: f32) -> f32 {
        let scaled = self.font.as_scaled(PxScale::from(size));
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
    pub(crate) fn draw_layers(
        &self,
        pixmap: &mut Pixmap,
        text: &str,
        size: f32,
        layers: &[(f32, f32, Color)],
    ) -> f32 {
        let font = &self.font;
        let scaled = font.as_scaled(PxScale::from(size));
        let glyphs: Vec<_> = self
            .layout(text, size)
            .into_iter()
            .map(|g| (g, font.outline(g.id)))
            .collect();
        let scale_factor = scaled.scale_factor();
        let width = pixmap.width() as i32;
        let height = pixmap.height() as i32;
        let pixels = pixmap.pixels_mut();

        let mut advance = 0.0;
        for &(x, y, color) in layers {
            let baseline = y + scaled.ascent();
            let mut caret = x;
            for (placed, outline) in &glyphs {
                if let Some(outline) = outline {
                    let glyph = placed.id.with_scale_and_position(
                        size,
                        point(caret + placed.dx, baseline + placed.dy),
                    );
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
                caret += placed.advance;
            }
            advance = caret - x;
        }
        advance
    }
}

/// Whether `text` uses a script that only reads correctly after shaping
/// (joined or reordered glyphs, right-to-left order), which the plain glyph
/// layout here doesn't do: Hebrew, Arabic, Syriac, Thaana, N'Ko, the Indic
/// and Southeast Asian abugidas (not Thai or Lao, whose marks stack well
/// enough), Tibetan, Myanmar, Khmer, Mongolian, and Adlam.
pub fn needs_shaping(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c as u32,
            0x0590..=0x08FF // Hebrew, Arabic, Syriac, Thaana, N'Ko, Samaritan, Mandaic
                | 0x0900..=0x0DFF // Devanagari through Sinhala
                | 0x0F00..=0x109F // Tibetan, Myanmar
                | 0x1780..=0x18AF // Khmer, Mongolian
                | 0x1900..=0x194F // Limbu
                | 0x1A00..=0x1AAF // Buginese, Tai Tham
                | 0x1B00..=0x1C4F // Balinese, Sundanese, Batak, Lepcha
                | 0xA800..=0xA82F // Syloti Nagri
                | 0xA880..=0xA8FF // Saurashtra, Devanagari Extended
                | 0xA980..=0xA9FF // Javanese, Myanmar Extended-B
                | 0xAA00..=0xAA7F // Cham, Myanmar Extended-A
                | 0xAAE0..=0xAAFF // Meetei Mayek Extensions
                | 0xABC0..=0xABFF // Meetei Mayek
                | 0xFB1D..=0xFDFF // Hebrew and Arabic presentation forms
                | 0xFE70..=0xFEFE // Arabic presentation forms B
                | 0x11100..=0x1114F // Chakma
                | 0x1E900..=0x1E95F // Adlam
        )
    })
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
pub(crate) fn measure(text: &str, size: f32) -> f32 {
    TextFont::bundled().measure(text, size)
}

/// The line height (ascent + descent + line gap) at the given pixel size
/// (bundled font).
pub(crate) fn line_height(size: f32) -> f32 {
    TextFont::bundled().line_height(size)
}

/// Draw a single line of text with its top-left at `(x, y)` in the bundled
/// font, returning the advance width.
pub(crate) fn draw_text(
    pixmap: &mut Pixmap,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Color,
) -> f32 {
    TextFont::bundled().draw(pixmap, text, x, y, size, color)
}

/// Alpha-blend a straight `color` at `coverage` over a premultiplied pixel.
fn blend(dst: PremultipliedColorU8, color: Color, coverage: f32) -> PremultipliedColorU8 {
    let sa = color.alpha() * coverage;
    if sa <= 0.0 {
        return dst;
    }
    let inv = 1.0 - sa;
    let blend_ch = |src: f32, dst: u8| ((src * 255.0 * sa) + f32::from(dst) * inv).round() as u8;

    let r = blend_ch(color.red(), dst.red());
    let g = blend_ch(color.green(), dst.green());
    let b = blend_ch(color.blue(), dst.blue());
    let a = (sa * 255.0 + f32::from(dst.alpha()) * inv).round() as u8;
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

    #[cfg(feature = "shaping")]
    fn arabic() -> TextFont {
        let bytes = include_bytes!("../../assets/test-arabic.ttf").to_vec();
        TextFont::from_bytes(bytes, 0).expect("the test font loads")
    }

    #[test]
    fn scripts_that_need_shaping_are_told_apart() {
        assert!(needs_shaping("القاهرة") && needs_shaping("नई दिल्ली") && needs_shaping("תל אביב"));
        assert!(needs_shaping("ᠤᠯᠠᠭᠠᠨᠪᠠᠭᠠᠲᠤᠷ"));
        assert!(!needs_shaping("Seoul") && !needs_shaping("서울") && !needs_shaping("東京"));
        assert!(!needs_shaping("กรุงเทพ") && !needs_shaping("\u{feff}Seoul"));
    }

    #[test]
    #[cfg(feature = "shaping")]
    fn arabic_is_joined_and_laid_out_right_to_left() {
        let font = arabic();
        let text = "القاهرة";
        let plain: Vec<GlyphId> = text.chars().map(|c| font.font.glyph_id(c)).collect();
        let shaped: Vec<GlyphId> = font.layout(text, 20.0).iter().map(|g| g.id).collect();
        assert!(
            plain.iter().all(|id| id.0 != 0),
            "the font has every letter"
        );
        let mut reversed = plain.clone();
        reversed.reverse();
        assert_ne!(shaped, plain);
        assert_ne!(shaped, reversed, "letters take their joined forms");
        // The word ends (on the left) with the final form of its last letter.
        let last = font.font.glyph_id('ة');
        assert_ne!(shaped[0], last);
        let width = font.measure(text, 20.0);
        assert!((width - font.advances(text, 20.0).iter().sum::<f32>()).abs() < 1e-3);
        let mut pixmap = Pixmap::new(160, 40).unwrap();
        let drawn = font.draw(&mut pixmap, text, 10.0, 5.0, 20.0, Color::WHITE);
        assert!((drawn - width).abs() < 1e-3);
        assert!(pixmap.pixels().iter().any(|p| p.alpha() > 0));
        // Latin text in the same font keeps one glyph per letter.
        assert_eq!(font.advances("ABC", 20.0).len(), 3);
    }

    #[test]
    #[cfg(feature = "shaping")]
    fn digits_and_latin_keep_their_order_inside_right_to_left_text() {
        let font = arabic();
        let ids =
            |text: &str| -> Vec<GlyphId> { font.layout(text, 20.0).iter().map(|g| g.id).collect() };
        let at =
            |glyphs: &[GlyphId], c: char| glyphs.iter().position(|&id| id == font.font.glyph_id(c));
        // "26 July Street": the number reads 2 then 6, left to right, though
        // the words around it run right to left.
        let street = ids("شارع 26 يوليو");
        assert!(at(&street, '2').unwrap() < at(&street, '6').unwrap());
        // A Latin word first in a mostly Arabic name stays on the left,
        // spelled forward, and the Arabic word to its right reads right to
        // left as on its own.
        let route = ids("Route 9 الطريق");
        assert_eq!(route[0], font.font.glyph_id('R'));
        assert!(at(&route, 'o').unwrap() < at(&route, 'u').unwrap());
        let alone = ids("الطريق");
        assert_eq!(&route[route.len() - alone.len()..], &alone[..]);
        // Shaping is cached per text, whatever the size.
        assert_eq!(
            font.advances("الطريق", 40.0)[0],
            2.0 * font.advances("الطريق", 20.0)[0]
        );
    }

    #[test]
    #[cfg(feature = "shaping")]
    fn turned_glyphs_follow_the_shaped_layout() {
        let font = arabic();
        let text = "بيروت";
        let advances = font.advances(text, 24.0);
        let mut x = 10.0;
        let glyphs: Vec<(f32, f32, f32)> = advances
            .iter()
            .map(|a| {
                let center = x + a / 2.0;
                x += a;
                (center, 20.0, 0.0)
            })
            .collect();
        let mut turned = Pixmap::new(140, 40).unwrap();
        font.draw_turned(&mut turned, text, 24.0, &glyphs, Color::WHITE, None);
        assert!(turned.pixels().iter().any(|p| p.alpha() > 0));
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
