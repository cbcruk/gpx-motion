//! The scene as data for drawing it with WebGL (e.g. three.js) instead of
//! rendering frames here: the terrain mesh and its texture, the track, the
//! marker, and the camera along the orbit.
//!
//! Positions are world pixels at the camera's zoom (x east, y south, `h` up)
//! relative to the scene origin: the terrain's top-left corner, or the
//! camera's starting target without terrain.

use gpx_motion_core::{
    draw_profile, fog_band, marker_vertex, track_vertices, Camera, TrackVertex, TERRAIN_CLEARANCE,
};
use tiny_skia::{Color, Pixmap};
use wasm_bindgen::prelude::*;

use super::SceneRenderer;
use crate::to_straight_rgba;

/// Floats per vertex in [`SceneRenderer::gl_track`].
const TRACK_STRIDE: usize = 8;

/// The terrain mesh: a grid of heights over the scene origin with the
/// hill-shaded map draped over it.
#[wasm_bindgen]
pub struct GlTerrain {
    cols: u32,
    rows: u32,
    cell_x: f64,
    cell_y: f64,
    width: u32,
    height: u32,
    heights: Vec<f32>,
    texture: Vec<u8>,
}

#[wasm_bindgen]
impl GlTerrain {
    /// Grid cells across; the grid has `cols + 1` points per row.
    #[wasm_bindgen(getter)]
    pub fn cols(&self) -> u32 {
        self.cols
    }

    /// Grid cells down; the grid has `rows + 1` rows of points.
    #[wasm_bindgen(getter)]
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// Cell width in world pixels.
    #[wasm_bindgen(getter, js_name = cellX)]
    pub fn cell_x(&self) -> f64 {
        self.cell_x
    }

    /// Cell height in world pixels.
    #[wasm_bindgen(getter, js_name = cellY)]
    pub fn cell_y(&self) -> f64 {
        self.cell_y
    }

    /// Texture width in texels, one per world pixel.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Texture height in texels, one per world pixel.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Height in world pixels of each grid point, row by row from the
    /// top-left. Split each cell along its top-left to bottom-right diagonal
    /// so the track, which stands on those triangles, meets the surface.
    pub fn heights(&self) -> Vec<f32> {
        self.heights.clone()
    }

    /// The hill-shaded map as straight RGBA, top row first.
    pub fn texture(&self) -> Vec<u8> {
        self.texture.clone()
    }
}

#[wasm_bindgen]
impl SceneRenderer {
    /// The terrain built by [`SceneRenderer::set_terrain_texture`], or
    /// `undefined` before (or without) one. Large: call once per build.
    #[wasm_bindgen(js_name = glTerrain)]
    pub fn gl_terrain(&self) -> Option<GlTerrain> {
        let terrain = self.terrain.as_ref()?;
        let (cols, rows) = terrain.grid_size();
        let (cell_x, cell_y) = terrain.cell();
        let texture = terrain.texture();
        Some(GlTerrain {
            cols: cols as u32,
            rows: rows as u32,
            cell_x,
            cell_y,
            width: texture.width(),
            height: texture.height(),
            heights: terrain.heights().iter().map(|&h| h as f32).collect(),
            texture: to_straight_rgba(texture),
        })
    }

    /// The whole track, [`TRACK_STRIDE`] floats per vertex: `x`, `y`,
    /// `lift` (relief height), `ground` (terrain height under it, `0`
    /// without terrain), progress `u`, and the segment's `r`, `g`, `b`
    /// (`0..=1`). The track stands at `max(lift, ground)` with terrain shown.
    #[wasm_bindgen(js_name = glTrack)]
    pub fn gl_track(&self) -> Vec<f32> {
        let vertices = track_vertices(
            &self.timeline,
            &self.camera,
            self.relief,
            self.terrain.as_ref(),
            &self.style,
        );
        let mut out = Vec::with_capacity(vertices.len() * TRACK_STRIDE);
        for v in vertices {
            out.extend(self.local(&v));
            out.extend([v.u as f32, v.color.red(), v.color.green(), v.color.blue()]);
        }
        out
    }

    /// The marker at progress `u`: `x`, `y`, `lift`, `ground`, as in
    /// [`SceneRenderer::gl_track`].
    #[wasm_bindgen(js_name = glMarker)]
    pub fn gl_marker(&self, u: f64) -> Vec<f32> {
        let v = marker_vertex(
            &self.timeline,
            &self.camera,
            self.relief,
            self.terrain.as_ref(),
            u,
            &self.style,
        );
        self.local(&v).to_vec()
    }

    /// The camera turned `turn` (`0..=1`) of the way along the orbit, kept
    /// above the shown terrain: eye `x`, `y`, `h`; target `x`, `y` (on the
    /// ground); vertical field of view in degrees; distance to the target;
    /// and the fog band's screen rows (horizon, end), `NaN` without fog.
    #[wasm_bindgen(js_name = glCamera)]
    pub fn gl_camera(&self, turn: f64) -> Vec<f64> {
        let camera = self.gl_camera_at(turn);
        let (origin_x, origin_y) = self.gl_origin();
        let (ex, ey, eh) = camera.eye();
        let (tx, ty) = camera.target();
        let (horizon, fog_end) =
            fog_band(&camera, self.style.height).unwrap_or((f64::NAN, f64::NAN));
        vec![
            ex - origin_x,
            ey - origin_y,
            eh,
            tx - origin_x,
            ty - origin_y,
            camera.fov_deg(),
            camera.distance,
            horizon,
            fog_end,
        ]
    }

    /// The elevation profile panel at progress `u` as straight RGBA, as wide
    /// as the viewport and [`SceneRenderer::profile_height`] tall, to
    /// overlay along the bottom edge; transparent around the panel.
    #[wasm_bindgen(js_name = renderProfile)]
    pub fn render_profile(&self, u: f64) -> Vec<u8> {
        let Some(mut pixmap) = Pixmap::new(self.style.width, self.profile_height()) else {
            return Vec::new();
        };
        draw_profile(&mut pixmap, &self.timeline, u, &self.profile, &self.style);
        to_straight_rgba(&pixmap)
    }

    /// Height in pixels of [`SceneRenderer::render_profile`]'s image.
    #[wasm_bindgen(getter, js_name = profileHeight)]
    pub fn profile_height(&self) -> u32 {
        (self.profile.height + 2.0 * self.profile.margin).ceil() as u32
    }

    /// Width of the track line in pixels.
    #[wasm_bindgen(getter, js_name = tailWidth)]
    pub fn tail_width(&self) -> f32 {
        self.style.tail_width
    }

    /// Radius of the marker in pixels.
    #[wasm_bindgen(getter, js_name = markerRadius)]
    pub fn marker_radius(&self) -> f32 {
        self.style.marker_radius
    }

    /// The marker's color as `r`, `g`, `b`, `a` (`0..=1`).
    #[wasm_bindgen(getter, js_name = markerColor)]
    pub fn marker_color(&self) -> Vec<f32> {
        rgba(self.style.marker_color)
    }

    /// The map style's background (and fog) color as `r`, `g`, `b`, `a`
    /// (`0..=1`).
    #[wasm_bindgen(getter)]
    pub fn background(&self) -> Vec<f32> {
        rgba(self.map_style.background)
    }
}

impl SceneRenderer {
    /// World pixels the GL positions are relative to.
    fn gl_origin(&self) -> (f64, f64) {
        match (&self.terrain, self.terrain_area) {
            (Some(terrain), _) => terrain.origin(),
            (None, Some(area)) => area.origin,
            (None, None) => self.camera_at(0.0).target(),
        }
    }

    fn gl_camera_at(&self, turn: f64) -> Camera {
        let camera = self.camera_at(turn);
        match self.shown_terrain() {
            Some(terrain) => camera.with_min_altitude(terrain.max_height() * TERRAIN_CLEARANCE),
            None => camera,
        }
    }

    fn local(&self, v: &TrackVertex) -> [f32; 4] {
        let (origin_x, origin_y) = self.gl_origin();
        [
            (v.x - origin_x) as f32,
            (v.y - origin_y) as f32,
            v.lift as f32,
            v.ground as f32,
        ]
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

    /// A Terrarium tile rising 512–767 m from west to east.
    fn slope_tile() -> Vec<u8> {
        (0..256 * 256)
            .flat_map(|i| [130, (i % 256) as u8, 0, 255])
            .collect()
    }

    fn with_terrain() -> SceneRenderer {
        let mut scene = scene();
        assert!(scene.enable_terrain());
        for tile in scene.terrain_tiles() {
            scene.add_terrain_tile(&tile, &slope_tile());
        }
        let texture = scene.render_terrain_texture(14).unwrap_or_default();
        assert!(scene.set_terrain_texture(texture).is_ok());
        scene
    }

    #[test]
    fn track_and_marker_are_relative_to_the_origin() {
        let scene = scene();
        let track = scene.gl_track();
        assert_eq!(track.len() % TRACK_STRIDE, 0);
        let vertices: Vec<&[f32]> = track.chunks(TRACK_STRIDE).collect();
        assert!(vertices.len() >= 2);
        assert_eq!(vertices[0][4], 0.0);
        assert_eq!(vertices[vertices.len() - 1][4], 1.0);
        assert!(vertices.iter().all(|v| v[3] == 0.0 && v[0].abs() < 2000.0));
        let marker = scene.gl_marker(0.0);
        assert_eq!(&marker[..], &vertices[0][..4]);
        // The camera's start target is the origin without terrain.
        let camera = scene.gl_camera(0.0);
        assert!(camera[3].abs() < 1e-9 && camera[4].abs() < 1e-9);
    }

    #[test]
    fn terrain_mesh_matches_the_track_ground_and_lifts_the_camera() {
        let flat = scene();
        let scene = with_terrain();
        assert!(flat.gl_terrain().is_none());
        let Some(terrain) = scene.gl_terrain() else {
            panic!("terrain should be built");
        };
        let points = ((terrain.cols() + 1) * (terrain.rows() + 1)) as usize;
        assert_eq!(terrain.heights().len(), points);
        let texels = (terrain.width() * terrain.height() * 4) as usize;
        assert_eq!(terrain.texture().len(), texels);
        let track = scene.gl_track();
        assert!(
            track.chunks(TRACK_STRIDE).any(|v| v[3] > 0.0),
            "stands on the slope"
        );
        assert!(track
            .chunks(TRACK_STRIDE)
            .all(|v| v[0] >= 0.0 && v[1] >= 0.0));

        let top = terrain.heights().iter().copied().fold(0.0, f32::max) as f64;
        for turn in [0.0, 0.5] {
            let eye_h = scene.gl_camera(turn)[2];
            assert!(eye_h >= top * TERRAIN_CLEARANCE - 1e-6, "turn {turn}");
        }
        let mut hidden = with_terrain();
        hidden.set_terrain_visible(false);
        assert_eq!(hidden.gl_camera(0.3)[..6], flat_camera(&hidden, 0.3)[..6]);
    }

    fn flat_camera(scene: &SceneRenderer, turn: f64) -> Vec<f64> {
        let camera = scene.camera_at(turn);
        let (ox, oy) = scene.gl_origin();
        let (ex, ey, eh) = camera.eye();
        let (tx, ty) = camera.target();
        vec![ex - ox, ey - oy, eh, tx - ox, ty - oy, camera.fov_deg()]
    }

    #[test]
    fn profile_strip_has_the_panel_and_clear_margins() {
        let scene = scene();
        let strip = scene.render_profile(0.5);
        let (w, h) = (480usize, scene.profile_height() as usize);
        assert_eq!(strip.len(), w * h * 4);
        assert_eq!(strip[3], 0, "the corner stays clear");
        let middle = ((h / 2) * w + w / 2) * 4;
        assert!(strip[middle + 3] > 0, "the panel is drawn");
    }
}
