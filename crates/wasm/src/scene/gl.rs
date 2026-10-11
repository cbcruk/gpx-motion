//! The scene as data for drawing it with WebGL (e.g. three.js) instead of
//! rendering frames here: the terrain mesh and its texture, the track, the
//! marker, and the camera along the orbit.
//!
//! Positions are world pixels at the camera's zoom (x east, y south, `h` up)
//! relative to the scene origin: the terrain's top-left corner, or the
//! camera's starting target without terrain.

use gpx_motion_core::{
    draw_profile, fog_band, ground_tiles, marker_vertex, track_vertices, Camera, GroundDetail,
    Terrain, TrackVertex, TERRAIN_CLEARANCE,
};
use tiny_skia::{Color, Pixmap};
use wasm_bindgen::prelude::*;

use super::SceneRenderer;
use crate::ground::TEXTURE_PX;
use crate::to_straight_rgba;

/// Floats per vertex in [`SceneRenderer::gl_track`].
const TRACK_STRIDE: usize = 8;

/// Numbers per tile in [`SceneRenderer::gl_ground`].
const GROUND_STRIDE: usize = 5;
/// Most tiles in a ground tree, inner ones included.
const MAX_GROUND_TILES: usize = 128;
/// Finest ground tile zoom (sources overzoom past their own).
const MAX_GROUND_ZOOM: u32 = 18;
/// Most cells across one ground tile's mesh.
const MAX_GROUND_CELLS: f64 = 64.0;

#[wasm_bindgen]
impl SceneRenderer {
    /// Build the terrain from the DEM tiles added so far, without a map
    /// texture (the WebGL scene drapes its own map tiles over it; see
    /// [`SceneRenderer::gl_ground_mesh`]). Errors before
    /// [`SceneRenderer::enable_terrain`].
    #[wasm_bindgen(js_name = setTerrainHeights)]
    pub fn set_terrain_heights(&mut self) -> Result<(), JsError> {
        let (Some(area), Some(dem)) = (self.terrain_area, &self.dem) else {
            return Err(JsError::new("terrain is not enabled"));
        };
        let size = (area.width, area.height);
        self.terrain = Some(Terrain::bare(
            dem,
            &self.relief,
            area.zoom,
            area.origin,
            size,
        ));
        Ok(())
    }

    /// The map tiles to drape over the ground for the GL camera turned
    /// `turn` (`0..=1`) along the orbit, drawn at `pixelRatio` device pixels
    /// per scene pixel: finer where the ground is nearer, coarser far away.
    /// [`GROUND_STRIDE`] numbers per tile: `zoom`, `x`, `y`, the index of
    /// the tile it splits plus one (`0` for a root), and `1` when it is split
    /// further (else `0`, a leaf to draw). Each split tile comes before its
    /// children.
    #[wasm_bindgen(js_name = glGround)]
    pub fn gl_ground(&self, turn: f64, pixel_ratio: f64) -> Vec<u32> {
        let detail = GroundDetail {
            texture_px: TEXTURE_PX as f64,
            pixel_ratio: pixel_ratio.max(1.0),
            max_tiles: MAX_GROUND_TILES,
            max_zoom: MAX_GROUND_ZOOM,
        };
        let tiles = ground_tiles(&self.gl_camera_at(turn), &detail);
        let mut out = Vec::with_capacity(tiles.len() * GROUND_STRIDE);
        for t in tiles {
            let parent = t.parent.map_or(0, |p| p as u32 + 1);
            out.extend([t.id.zoom, t.id.x, t.id.y, parent, t.split as u32]);
        }
        out
    }

    /// The ground mesh of map tile `zoom/x/y`: its top-left corner `x`, `y`
    /// and side in scene world pixels, the cells `n` across, then the height
    /// and the hill-shading factor of each of its `(n + 1)²` grid points, row
    /// by row (heights first). Flat (`n` = 1) away from the terrain; over it,
    /// cells about as fine as the terrain's.
    #[wasm_bindgen(js_name = glGroundMesh)]
    pub fn gl_ground_mesh(&self, zoom: u32, x: u32, y: u32) -> Vec<f32> {
        let side = 256.0 * 2f64.powf(self.camera.zoom - zoom as f64);
        let (x0, y0) = (x as f64 * side, y as f64 * side);
        let terrain = self.terrain.as_ref().filter(|t| {
            let (ox, oy) = t.origin();
            let (cols, rows) = t.grid_size();
            let (cw, ch) = t.cell();
            let (w, h) = (cols as f64 * cw, rows as f64 * ch);
            x0 < ox + w && x0 + side > ox && y0 < oy + h && y0 + side > oy
        });
        let n = terrain.map_or(1, |t| {
            let (cw, ch) = t.cell();
            (side / cw.min(ch)).ceil().clamp(1.0, MAX_GROUND_CELLS) as usize
        });
        let (ox, oy) = self.gl_origin();
        let mut out = Vec::with_capacity(4 + 2 * (n + 1) * (n + 1));
        out.extend([(x0 - ox) as f32, (y0 - oy) as f32, side as f32, n as f32]);
        let points = || {
            (0..=n).flat_map(move |r| {
                (0..=n).map(move |c| {
                    let step = side / n as f64;
                    (x0 + c as f64 * step, y0 + r as f64 * step)
                })
            })
        };
        let flat = Terrain::flat_light();
        out.extend(
            points().map(|(px, py)| terrain.map_or(0.0, |t| t.height_at_world(px, py)) as f32),
        );
        out.extend(
            points().map(|(px, py)| terrain.map_or(flat, |t| t.light_at_world(px, py)) as f32),
        );
        out
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
    /// [`SceneRenderer::gl_track`], then `1` when the shown terrain hides it
    /// from the camera at `turn` (else `0`), so the whole marker is drawn
    /// faintly without its shadow and drop line, as the CPU renderer does.
    #[wasm_bindgen(js_name = glMarker)]
    pub fn gl_marker(&self, u: f64, turn: f64) -> Vec<f32> {
        let v = marker_vertex(
            &self.timeline,
            &self.camera,
            self.relief,
            self.terrain.as_ref(),
            u,
            &self.style,
        );
        let hidden = self.shown_terrain().is_some_and(|terrain| {
            let top = v.lift.max(v.ground);
            terrain.hides_point(&self.gl_camera_at(turn), v.x, v.y, top)
        });
        let mut out = self.local(&v).to_vec();
        out.push(if hidden { 1.0 } else { 0.0 });
        out
    }

    /// The camera turned `turn` (`0..=1`) of the way along the orbit, kept
    /// above the shown terrain: eye `x`, `y`, `h`; target `x`, `y` (on the
    /// ground); vertical field of view in degrees; distance to the target;
    /// the fog band's screen rows (horizon, end), `NaN` without fog; and the
    /// bearing in degrees (the compass direction up the screen, which also
    /// orients a camera looking straight down); and the farthest view depth
    /// whose ground is drawn.
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
            camera.bearing_deg(),
            camera.far_depth(),
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

    pub(super) fn gl_camera_at(&self, turn: f64) -> Camera {
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
        assert!(scene.set_terrain_heights().is_ok());
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
        let marker = scene.gl_marker(0.0, 0.0);
        assert_eq!(&marker[..4], &vertices[0][..4]);
        assert_eq!(marker[4], 0.0, "nothing hides it without terrain");
        // The camera's start target is the origin without terrain.
        let camera = scene.gl_camera(0.0);
        assert!(camera[3].abs() < 1e-9 && camera[4].abs() < 1e-9);
    }

    #[test]
    fn ground_meshes_follow_the_terrain_and_lift_the_camera() {
        let flat = scene();
        let scene = with_terrain();
        let tree = scene.gl_ground(0.2, 2.0);
        assert_eq!(tree.len() % GROUND_STRIDE, 0);
        let tiles: Vec<&[u32]> = tree.chunks(GROUND_STRIDE).collect();
        assert!(tiles.len() > 4 && tiles.len() <= MAX_GROUND_TILES);
        for (i, t) in tiles.iter().enumerate() {
            if t[3] > 0 {
                let parent = tiles[t[3] as usize - 1];
                assert!((t[3] as usize - 1) < i && parent[4] == 1);
                assert_eq!(parent[0] + 1, t[0]);
            }
        }
        let mut top = 0.0f32;
        let mut raised = 0;
        for t in tiles.iter().filter(|t| t[4] == 0) {
            let mesh = scene.gl_ground_mesh(t[0], t[1], t[2]);
            let n = mesh[3] as usize;
            assert_eq!(mesh.len(), 4 + 2 * (n + 1) * (n + 1));
            let heights = &mesh[4..4 + (n + 1) * (n + 1)];
            top = heights.iter().copied().fold(top, f32::max);
            if n > 1 {
                raised += 1;
            }
            let flat_mesh = flat.gl_ground_mesh(t[0], t[1], t[2]);
            assert_eq!(flat_mesh[3], 1.0, "no terrain, no relief");
        }
        assert!(raised > 0 && top > 0.0);
        let track = scene.gl_track();
        assert!(
            track.chunks(TRACK_STRIDE).any(|v| v[3] > 0.0),
            "stands on the slope"
        );
        for turn in [0.0, 0.5] {
            let eye_h = scene.gl_camera(turn)[2];
            assert!(
                eye_h >= top as f64 * TERRAIN_CLEARANCE - 1e-3,
                "turn {turn}"
            );
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
