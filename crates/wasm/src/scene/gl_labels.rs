//! Map labels for the WebGL scene: placed here for the GL camera (with the
//! same memory, fades, and terrain hiding as the CPU renderer) and handed
//! over as data for the page to draw.

use gpx_motion_core::{
    halo_radius, needs_shaping, track_outline_3d, Keepout, LabelSources, PlacedLabel, TextFont,
    TileId, TilePlacement, VectorTile,
};
use tiny_skia::{Color, Rect};
use wasm_bindgen::prelude::*;

use super::SceneRenderer;

/// Views a label takes to fade in or out (about 0.4 s at 60 frames a second).
pub(super) const FADE_FRAMES: u32 = 24;

/// Floats per label in [`GlLabels::labels`].
const LABEL_STRIDE: usize = 9;

/// Labels placed for one view, in the scene's (CSS) pixels.
#[wasm_bindgen]
pub struct GlLabels {
    texts: Vec<String>,
    labels: Vec<f32>,
    glyphs: Vec<f32>,
    color: Vec<f32>,
    halo: Vec<f32>,
}

#[wasm_bindgen]
impl GlLabels {
    /// Each label's text.
    pub fn texts(&self) -> Vec<String> {
        self.texts.clone()
    }

    /// [`LABEL_STRIDE`] floats per label: the left end of its baseline `x`,
    /// `y` (point labels); the CSS font size; opacity (`0..=1`); halo
    /// width; how far below a glyph's center its baseline sits (line
    /// labels); the first glyph and glyph count in [`GlLabels::glyphs`]
    /// (`0` for a point label); and `1` for a line label in a shaped script
    /// (whose glyphs are not one per character, see `TextFont`), to draw
    /// whole along the line between its first and last glyph, else `0`.
    pub fn labels(&self) -> Vec<f32> {
        self.labels.clone()
    }

    /// Glyphs of the labels that follow a line, three floats each: the
    /// glyph's center `x`, `y` and its angle (radians, clockwise from the x
    /// axis), one per character of the text unless the label is to be drawn
    /// whole.
    pub fn glyphs(&self) -> Vec<f32> {
        self.glyphs.clone()
    }

    /// Text color as `r`, `g`, `b`, `a` (`0..=1`).
    #[wasm_bindgen(getter)]
    pub fn color(&self) -> Vec<f32> {
        self.color.clone()
    }

    /// Halo color as `r`, `g`, `b`, `a` (`0..=1`).
    #[wasm_bindgen(getter)]
    pub fn halo(&self) -> Vec<f32> {
        self.halo.clone()
    }
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Map labels for the GL camera turned `turn` (`0..=1`) of the way along
    /// the orbit, or `undefined` without a label font
    /// ([`SceneRenderer::set_labels`]). Label tiles come from
    /// [`SceneRenderer::add_label_tile`] (those of
    /// [`SceneRenderer::label_tiles`]).
    ///
    /// Call once per drawn frame: labels fade in and out over frames, and
    /// ones shown last frame keep their place.
    #[wasm_bindgen(js_name = glLabels)]
    pub fn gl_labels(&self, turn: f64) -> Option<GlLabels> {
        let labels = self.labels.as_ref()?;
        let style = labels.style(self.map_style.background, self.map_style.pixel_ratio);
        let mut cache = self.gl_label_sources.borrow_mut();
        let sources = cache.get_or_insert_with(|| {
            let zoom = self.camera.zoom;
            let tiles: Vec<(TilePlacement, &VectorTile)> = self
                .label_tiles
                .iter()
                .map(|(id, tile)| (world_placement(*id, zoom), tile))
                .collect();
            LabelSources::collect(&tiles, &style)
        });
        let camera = self.gl_camera_at(turn);
        let (width, height) = (self.style.width as f32, self.style.height as f32);
        let panel = self.bottom_inset as f32;
        let rects: Vec<Rect> = Rect::from_xywh(0.0, height - panel, width, panel)
            .into_iter()
            .collect();
        let terrain = self.shown_terrain();
        let (lines, areas) = track_outline_3d(&camera, &self.points, self.relief, terrain);
        let ratio = self.map_style.pixel_ratio;
        let keepout = Keepout::around(&rects, lines, areas, self.style.tail_width, ratio);
        let canvas = (self.style.width, self.style.height);
        let mut memory = self.gl_label_memory.borrow_mut();
        let placed =
            sources.place_3d_steady(&camera, &style, canvas, &keepout, terrain, &mut memory);
        let mut out = GlLabels {
            texts: Vec::with_capacity(placed.len()),
            labels: Vec::with_capacity(placed.len() * LABEL_STRIDE),
            glyphs: Vec::new(),
            color: rgba(style.color),
            halo: rgba(style.halo),
        };
        for label in placed {
            push(&mut out, &label, &style.font);
        }
        Some(out)
    }
}

fn push(out: &mut GlLabels, label: &PlacedLabel, font: &TextFont) {
    let first = (out.glyphs.len() / 3) as f32;
    for &(x, y, angle) in &label.glyphs {
        out.glyphs.extend([x, y, angle]);
    }
    out.labels.extend([
        label.x,
        label.y + font.ascent(label.size),
        font.em_size(label.size),
        label.opacity,
        halo_radius(label.size),
        font.rise(label.size),
        first,
        label.glyphs.len() as f32,
        if !label.glyphs.is_empty() && needs_shaping(&label.text) {
            1.0
        } else {
            0.0
        },
    ]);
    out.texts.push(label.text.clone());
}

/// Where tile `id` lies in world pixels at `zoom`, as 3D label placement
/// expects.
fn world_placement(id: TileId, zoom: f64) -> TilePlacement {
    let size = 256.0 * 2f64.powf(zoom - id.zoom as f64);
    TilePlacement {
        id,
        dx: id.x as f64 * size,
        dy: id.y as f64 * size,
        size,
    }
}

fn rgba(color: Color) -> Vec<f32> {
    vec![color.red(), color.green(), color.blue(), color.alpha()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scene() -> SceneRenderer {
        let gpx = include_str!("../../../../samples/namsan.gpx");
        let Ok(scene) = SceneRenderer::new(gpx, 480, 360, 55.0, 20.0, 360.0, None) else {
            panic!("scene should build");
        };
        scene
    }

    #[test]
    fn world_placement_matches_the_mercator_grid() {
        let id = TileId {
            zoom: 12,
            x: 3500,
            y: 1600,
        };
        let p = world_placement(id, 13.5);
        assert!((p.size - 256.0 * 2f64.powf(1.5)).abs() < 1e-9);
        assert!((p.dx - 3500.0 * p.size).abs() < 1e-6);
        assert!((p.dy - 1600.0 * p.size).abs() < 1e-6);
    }

    #[test]
    fn labels_need_a_font_and_start_empty_without_tiles() {
        let mut scene = scene();
        assert!(scene.gl_labels(0.0).is_none());
        let font = include_bytes!("../../../core/assets/font.ttf").to_vec();
        assert!(scene.set_labels(font, Some(14.0), None).is_ok());
        let Some(labels) = scene.gl_labels(0.0) else {
            panic!("labels should be on");
        };
        assert!(labels.texts().is_empty() && labels.labels().is_empty());
        assert_eq!(labels.color().len(), 4);
        assert_eq!(labels.halo().len(), 4);
    }
}
