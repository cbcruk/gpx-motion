//! The scene as data for drawing it with WebGL (e.g. three.js) instead of
//! rendering frames here: the terrain mesh and its texture, the track, the
//! marker, and the camera along the orbit.
//!
//! Positions are world pixels at the camera's zoom (x east, y south, `h` up)
//! relative to the scene origin: the terrain's top-left corner, or the
//! camera's starting target without terrain.

use gpx_motion_core::{
    draw_profile, fog_band, ground_tile_tree, marker_vertex, track_vertices, GroundDetail, Terrain,
    TrackVertex,
};
use tiny_skia::{Color, Pixmap};
use wasm_bindgen::prelude::*;

use super::SceneRenderer;
use crate::ground::TEXTURE_PX;
use crate::layout::{
    BuildingField, CameraField, GroundMeshField, GroundNodeField, MarkerField, TrackField,
    BUILDING_STRIDE, CAMERA_LEN, GROUND_STRIDE, MARKER_LEN, TRACK_STRIDE,
};
use crate::to_straight_rgba;

/// Most tiles in a ground tree, inner ones included.
const MAX_GROUND_TILES: usize = 128;
/// Finest ground tile zoom (sources overzoom past their own).
const MAX_GROUND_ZOOM: u32 = 18;
/// Most terrain grid lines a ground tile's mesh takes along each side, plus
/// the midway lines between them (far, coarse tiles skip some; near ones
/// keep every line).
const MAX_TILE_LINES: usize = 64;

#[wasm_bindgen]
impl SceneRenderer {
    /// The map tiles to drape over the ground for the GL camera turned
    /// `turn` (`0..=1`) along the orbit at progress `u`, drawn at `pixelRatio` device pixels
    /// per scene pixel: finer where the ground is nearer, coarser far away.
    /// [`GROUND_STRIDE`] numbers per tile, laid out by [`GroundNodeField`]:
    /// `zoom`, `x`, `y`, the index of the tile it splits plus one (`0` for a
    /// root), and `1` when it is split further (else `0`, a leaf to draw).
    /// Each split tile comes before its children.
    #[wasm_bindgen(js_name = glGround)]
    pub fn gl_ground(&self, turn: f64, u: f64, pixel_ratio: f64) -> Vec<u32> {
        let detail = GroundDetail {
            texture_px: TEXTURE_PX as f64,
            pixel_ratio: pixel_ratio.max(1.0),
            max_tiles: MAX_GROUND_TILES,
            max_zoom: MAX_GROUND_ZOOM,
        };
        let tiles = ground_tile_tree(&self.gl_camera_at(turn, u), &detail);
        let mut out = Vec::with_capacity(tiles.len() * GROUND_STRIDE);
        for t in tiles {
            let mut node = [0; GROUND_STRIDE];
            node[GroundNodeField::Zoom as usize] = t.id.zoom;
            node[GroundNodeField::X as usize] = t.id.x;
            node[GroundNodeField::Y as usize] = t.id.y;
            node[GroundNodeField::Parent as usize] = t.parent.map_or(0, |p| p as u32 + 1);
            node[GroundNodeField::Split as usize] = u32::from(t.split);
            out.extend(node);
        }
        out
    }

    /// The ground mesh of map tile `zoom/x/y`, its header laid out by
    /// [`GroundMeshField`]: its top-left corner `x`, `y` and side in scene
    /// world pixels; the grid cells across, `nx`, and down, `ny`; then each
    /// line's offset from the corner (`nx + 1` across, then
    /// `ny + 1` down); then the height and the hill-shading factor of each
    /// grid point, row by row (heights first). Split each grid cell along
    /// its top-left to bottom-right diagonal.
    ///
    /// Flat (one cell) away from the terrain. Over it, the grid has every
    /// terrain grid line inside the tile, so heights match the terrain (and
    /// the track standing on it) at the terrain's own points and its border,
    /// plus lines between them at most half a terrain cell apart.
    #[wasm_bindgen(js_name = glGroundMesh)]
    pub fn gl_ground_mesh(&self, zoom: u32, x: u32, y: u32) -> Vec<f32> {
        let side = 256.0 * 2f64.powf(self.view.fitted().zoom - zoom as f64);
        let (x0, y0) = (x as f64 * side, y as f64 * side);
        let (xs, ys) = match self.terrain.built() {
            Some(t) => {
                let (ox, oy) = t.origin();
                let (xs, ys) = t.grid_lines();
                (grid_lines(x0, side, ox, xs), grid_lines(y0, side, oy, ys))
            }
            None => (vec![0.0, side], vec![0.0, side]),
        };
        let (ox, oy) = self.gl_origin();
        let points = xs.len() * ys.len();
        let mut header = [0.0; GroundMeshField::Lines as usize];
        header[GroundMeshField::X as usize] = (x0 - ox) as f32;
        header[GroundMeshField::Y as usize] = (y0 - oy) as f32;
        header[GroundMeshField::Side as usize] = side as f32;
        header[GroundMeshField::CellsX as usize] = (xs.len() - 1) as f32;
        header[GroundMeshField::CellsY as usize] = (ys.len() - 1) as f32;
        let mut out = Vec::with_capacity(header.len() + xs.len() + ys.len() + 2 * points);
        out.extend(header);
        out.extend(xs.iter().map(|&v| v as f32));
        out.extend(ys.iter().map(|&v| v as f32));
        let grid = || {
            ys.iter()
                .flat_map(|&dy| xs.iter().map(move |&dx| (x0 + dx, y0 + dy)))
        };
        let terrain = self.terrain.built();
        out.extend(
            grid().map(|(px, py)| terrain.map_or(0.0, |t| t.height_at_world(px, py)) as f32),
        );
        let flat = Terrain::flat_light();
        out.extend(
            grid().map(|(px, py)| terrain.map_or(flat, |t| t.light_at_world(px, py)) as f32),
        );
        out
    }

    /// The buildings of ground tile `zoom/x/y` placed in the scene, from
    /// [`crate::GroundTiles::buildings`] data: [`BUILDING_STRIDE`] floats
    /// per corner laid out by [`BuildingField`], `x`, `y` in scene world pixels, the height above its
    /// building's foot, the foot's height (the lowest terrain under the
    /// building, `0` without terrain; scaled with the relief), and the
    /// face's straight `r`, `g`, `b`, `a`.
    #[wasm_bindgen(js_name = glBuildings)]
    pub fn gl_buildings(&self, zoom: u32, x: u32, y: u32, data: &[f32]) -> Vec<f32> {
        let scale = 2f64.powf(self.view.fitted().zoom - zoom as f64);
        let (ox, oy) = self.gl_origin();
        let (left, top) = (x as f64 * 256.0, y as f64 * 256.0);
        let terrain = self.terrain.built();
        let (corners, _) = data.as_chunks::<BUILDING_STRIDE>();
        let world = |c: &[f32; BUILDING_STRIDE]| {
            (
                (left + c[BuildingField::X as usize] as f64) * scale,
                (top + c[BuildingField::Y as usize] as f64) * scale,
            )
        };
        let mut out = Vec::with_capacity(data.len());
        let mut start = 0;
        while start < corners.len() {
            let prism = corners[start][BuildingField::Foot as usize];
            let end = corners[start..]
                .iter()
                .position(|c| c[BuildingField::Foot as usize] != prism)
                .map_or(corners.len(), |n| start + n);
            let building = &corners[start..end];
            let foot = terrain.map_or(0.0, |t| {
                building
                    .iter()
                    .map(|c| {
                        let (wx, wy) = world(c);
                        t.height_at_world(wx, wy)
                    })
                    .fold(f64::INFINITY, f64::min)
            });
            for c in building {
                let (wx, wy) = world(c);
                let mut corner = *c;
                corner[BuildingField::X as usize] = (wx - ox) as f32;
                corner[BuildingField::Y as usize] = (wy - oy) as f32;
                corner[BuildingField::Height as usize] =
                    (c[BuildingField::Height as usize] as f64 * scale) as f32;
                corner[BuildingField::Foot as usize] = foot as f32;
                out.extend(corner);
            }
            start = end;
        }
        out
    }

    /// The whole track, [`TRACK_STRIDE`] floats per vertex laid out by
    /// [`TrackField`]: `x`, `y`,
    /// `lift` (relief height), `ground` (terrain height under it, `0`
    /// without terrain), progress `u`, and the segment's `r`, `g`, `b`
    /// (`0..=1`). The track stands at `max(lift, ground)` with terrain shown.
    #[wasm_bindgen(js_name = glTrack)]
    pub fn gl_track(&self) -> Vec<f32> {
        let route = &self.route;
        let vertices = track_vertices(
            &route.timeline,
            self.view.fitted(),
            route.relief,
            self.terrain.built(),
            &self.style,
        );
        let mut out = Vec::with_capacity(vertices.len() * TRACK_STRIDE);
        for v in vertices {
            let (x, y, lift, ground) = self.local(&v);
            let mut vertex = [0.0; TRACK_STRIDE];
            vertex[TrackField::X as usize] = x;
            vertex[TrackField::Y as usize] = y;
            vertex[TrackField::Lift as usize] = lift;
            vertex[TrackField::Ground as usize] = ground;
            vertex[TrackField::U as usize] = v.u as f32;
            vertex[TrackField::R as usize] = v.color.red();
            vertex[TrackField::G as usize] = v.color.green();
            vertex[TrackField::B as usize] = v.color.blue();
            out.extend(vertex);
        }
        out
    }

    /// The marker at progress `u`, laid out by [`MarkerField`]: `x`, `y`,
    /// `lift`, `ground`, as in
    /// [`SceneRenderer::gl_track`], then `1` when the shown terrain hides it
    /// from the camera at `turn` and `u` (else `0`), so the whole marker is drawn
    /// faintly without its shadow and drop line, as the CPU renderer does.
    #[wasm_bindgen(js_name = glMarker)]
    pub fn gl_marker(&self, u: f64, turn: f64) -> Vec<f32> {
        let route = &self.route;
        let v = marker_vertex(
            &route.timeline,
            self.view.fitted(),
            route.relief,
            self.terrain.built(),
            u,
            &self.style,
        );
        let hidden = self.terrain.shown().is_some_and(|terrain| {
            let top = v.lift.max(v.ground);
            terrain.hides_point(&self.gl_camera_at(turn, u), v.x, v.y, top)
        });
        let (x, y, lift, ground) = self.local(&v);
        let mut out = vec![0.0; MARKER_LEN];
        out[MarkerField::X as usize] = x;
        out[MarkerField::Y as usize] = y;
        out[MarkerField::Lift as usize] = lift;
        out[MarkerField::Ground as usize] = ground;
        out[MarkerField::Hidden as usize] = if hidden { 1.0 } else { 0.0 };
        out
    }

    /// The camera turned `turn` (`0..=1`) of the way along the orbit at
    /// progress `u` (which moves it only while following the marker, see
    /// [`SceneRenderer::set_follow`]), kept above the shown terrain, laid out
    /// by [`CameraField`]: eye `x`, `y`, `h`; target `x`, `y` (on the
    /// ground); vertical field of view in degrees; distance to the target;
    /// the fog band's screen rows (horizon, end), `NaN` without fog; and the
    /// bearing in degrees (the compass direction up the screen, which also
    /// orients a camera looking straight down); and the farthest view depth
    /// whose ground is drawn.
    #[wasm_bindgen(js_name = glCamera)]
    pub fn gl_camera(&self, turn: f64, u: f64) -> Vec<f64> {
        let camera = self.gl_camera_at(turn, u);
        let (origin_x, origin_y) = self.gl_origin();
        let (ex, ey, eh) = camera.eye();
        let (tx, ty) = camera.target();
        let (horizon, fog_end) =
            fog_band(&camera, self.style.height).unwrap_or((f64::NAN, f64::NAN));
        let mut out = vec![0.0; CAMERA_LEN];
        out[CameraField::EyeX as usize] = ex - origin_x;
        out[CameraField::EyeY as usize] = ey - origin_y;
        out[CameraField::EyeH as usize] = eh;
        out[CameraField::TargetX as usize] = tx - origin_x;
        out[CameraField::TargetY as usize] = ty - origin_y;
        out[CameraField::Fov as usize] = camera.fov_deg();
        out[CameraField::Distance as usize] = camera.distance;
        out[CameraField::Horizon as usize] = horizon;
        out[CameraField::FogEnd as usize] = fog_end;
        out[CameraField::Bearing as usize] = camera.bearing_deg();
        out[CameraField::Far as usize] = camera.far_depth();
        out
    }

    /// The elevation profile panel at progress `u` as straight RGBA, as wide
    /// as the viewport and [`SceneRenderer::gl_profile_height`] tall, to
    /// overlay along the bottom edge; transparent around the panel.
    #[wasm_bindgen(js_name = glProfile)]
    pub fn gl_profile(&self, u: f64) -> Vec<u8> {
        let Some(mut pixmap) = Pixmap::new(self.style.width, self.gl_profile_height()) else {
            return Vec::new();
        };
        draw_profile(
            &mut pixmap,
            &self.route.timeline,
            u,
            &self.profile,
            &self.style,
        );
        to_straight_rgba(&pixmap)
    }

    /// Height in pixels of [`SceneRenderer::gl_profile`]'s image.
    #[wasm_bindgen(getter, js_name = glProfileHeight)]
    pub fn gl_profile_height(&self) -> u32 {
        (self.profile.height + 2.0 * self.profile.margin).ceil() as u32
    }

    /// Width of the track line in pixels.
    #[wasm_bindgen(getter, js_name = glTailWidth)]
    pub fn gl_tail_width(&self) -> f32 {
        self.style.tail_width
    }

    /// Radius of the marker in pixels.
    #[wasm_bindgen(getter, js_name = glMarkerRadius)]
    pub fn gl_marker_radius(&self) -> f32 {
        self.style.marker_radius
    }

    /// The marker's color as `r`, `g`, `b`, `a` (`0..=1`).
    #[wasm_bindgen(getter, js_name = glMarkerColor)]
    pub fn gl_marker_color(&self) -> Vec<f32> {
        rgba(self.style.marker_color)
    }

    /// The map style's background (and fog) color as `r`, `g`, `b`, `a`
    /// (`0..=1`).
    #[wasm_bindgen(getter, js_name = glBackground)]
    pub fn gl_background(&self) -> Vec<f32> {
        rgba(self.map_style.background)
    }
}

impl SceneRenderer {
    /// World pixels the GL positions are relative to: the terrain's
    /// top-left corner, else the framing camera's first target.
    fn gl_origin(&self) -> (f64, f64) {
        self.terrain
            .origin()
            .unwrap_or_else(|| self.view.start_target())
    }

    fn local(&self, v: &TrackVertex) -> (f32, f32, f32, f32) {
        let (origin_x, origin_y) = self.gl_origin();
        (
            (v.x - origin_x) as f32,
            (v.y - origin_y) as f32,
            v.lift as f32,
            v.ground as f32,
        )
    }
}

/// Offsets from `start` of the grid lines across a tile `side` long: its
/// ends, and every terrain grid line (`lines`, offsets from `origin`) and
/// midway line inside it. Halving the cells this way puts every mesh cell
/// inside one terrain triangle or across the terrain cell's diagonal along
/// its own, so the mesh of a tile keeping every line lies exactly on the
/// terrain, except in the clipped cells along the tile's edges (off by a
/// fraction of a pixel there). Just
/// the ends where the tile misses the terrain. A tile spanning more than
/// [`MAX_TILE_LINES`] terrain lines takes every few, so far coarse tiles
/// stay light (and lie only near the terrain).
fn grid_lines(start: f64, side: f64, origin: f64, lines: &[f64]) -> Vec<f64> {
    let inside = |v: &f64| (0.0..=side).contains(v);
    let count = lines
        .iter()
        .map(|&v| origin + v - start)
        .filter(inside)
        .count();
    // A tile spanning many terrain cells (far away, coarse) takes every few
    // lines: it covers few pixels.
    let stride = count.div_ceil(MAX_TILE_LINES).max(1);
    let kept: Vec<f64> = lines.iter().copied().step_by(stride).collect();
    let halves = kept
        .windows(2)
        .flat_map(|w| [w[0], (w[0] + w[1]) / 2.0])
        .chain(kept.last().copied());
    let mut out = vec![0.0];
    out.extend(halves.map(|v| origin + v - start).filter(inside));
    out.push(side);
    out.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    out
}

fn rgba(color: Color) -> Vec<f32> {
    vec![color.red(), color.green(), color.blue(), color.alpha()]
}

#[cfg(test)]
mod tests {
    use super::super::camera::{EYE_CLEARANCE, MAX_FOLLOW_ZOOM};
    use super::super::test_support::{namsan, with_terrain};
    use super::*;
    use gpx_motion_core::TERRAIN_CLEARANCE;

    fn scene() -> SceneRenderer {
        namsan(false)
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
        let camera = scene.gl_camera(0.0, 0.0);
        assert!(camera[3].abs() < 1e-9 && camera[4].abs() < 1e-9);
    }

    #[test]
    fn buildings_stand_on_the_lowest_ground_under_them() {
        let scene = with_terrain();
        // The zoom-16 tile under the camera's target, in the terrain.
        let zoom = 16;
        let scale = 2f64.powf(scene.view.fitted().zoom - zoom as f64);
        let (tx, ty) = scene.view.fitted().target();
        let (x, y) = ((tx / scale / 256.0) as u32, (ty / scale / 256.0) as u32);
        let (lx, ly) = (
            (tx / scale - x as f64 * 256.0) as f32,
            (ty / scale - y as f64 * 256.0) as f32,
        );
        // Two one-triangle buildings in the tile, 4 px tall at its zoom.
        let corner = |cx: f32, cy: f32, h: f32, prism: f32| [cx, cy, h, prism, 0.5, 0.5, 0.5, 0.8];
        let data: Vec<f32> = [
            corner(lx - 40.0, ly, 0.0, 0.0),
            corner(lx - 20.0, ly, 0.0, 0.0),
            corner(lx - 20.0, ly, 4.0, 0.0),
            corner(lx + 20.0, ly, 0.0, 1.0),
            corner(lx + 30.0, ly, 0.0, 1.0),
            corner(lx + 30.0, ly + 10.0, 4.0, 1.0),
        ]
        .concat();
        let out = scene.gl_buildings(zoom, x, y, &data);
        assert_eq!(out.len(), data.len());
        let corners: Vec<&[f32]> = out.chunks(BUILDING_STRIDE).collect();
        let (ox, oy) = scene.gl_origin();
        let world = |cx: f64, cy: f64| {
            (
                (x as f64 * 256.0 + cx) * scale,
                (y as f64 * 256.0 + cy) * scale,
            )
        };
        let Some(terrain) = scene.terrain.built() else {
            panic!("terrain is set");
        };
        // The slope rises eastward: each building stands at its west end.
        let (lx, ly) = (lx as f64, ly as f64);
        for (range, west) in [(0..3, (lx - 40.0, ly)), (3..6, (lx + 20.0, ly))] {
            let (wx, wy) = world(west.0, west.1);
            let foot = terrain.height_at_world(wx, wy) as f32;
            for c in &corners[range] {
                assert!((c[3] - foot).abs() < 1e-3, "{} vs {foot}", c[3]);
            }
        }
        assert!(corners[3][3] > corners[0][3]);
        let (wx, wy) = world(lx - 40.0, ly);
        assert!((corners[0][0] as f64 - (wx - ox)).abs() < 1e-2);
        assert!((corners[0][1] as f64 - (wy - oy)).abs() < 1e-2);
        assert!((corners[2][2] as f64 - 4.0 * scale).abs() < 1e-3);
        assert_eq!(&corners[2][4..], &[0.5, 0.5, 0.5, 0.8]);
    }

    #[test]
    fn following_cameras_aim_at_the_marker_up_close() {
        let mut scene = scene();
        let framed = scene.gl_camera(0.0, 0.3);
        assert_eq!(framed, scene.gl_camera(0.0, 0.7), "a framing camera stays");
        scene.set_follow(Some(30.0));
        assert_eq!(scene.follow_zoom(), Some(MAX_FOLLOW_ZOOM));
        scene.set_follow(Some(0.0));
        assert_eq!(scene.follow_zoom(), Some(scene.view.fitted().zoom));
        scene.set_follow(Some(scene.view.fitted().zoom + 2.0));
        assert!(scene.follow_zoom().is_some());
        for u in [0.3, 0.7] {
            let camera = scene.gl_camera(0.0, u);
            assert_ne!(camera, framed);
            let focal = scene.view.fitted().focal();
            // At least the zoom's distance, plus the way down to the target.
            assert!(camera[6] >= focal / 4.0 * (1.0 - 1e-9));
        }
        scene.set_follow(None);
        assert_eq!(scene.gl_camera(0.0, 0.3), framed);
    }

    /// The marker at progress `u` (scene world pixels) and its depth before
    /// `scene`'s following camera, and the depth the follow zoom asks for.
    fn marker_depth(scene: &SceneRenderer, u: f64) -> ((f64, f64, f64), f64, f64) {
        let Some(zoom) = scene.follow_zoom() else {
            panic!("following");
        };
        let (ox, oy) = scene.gl_origin();
        let m = scene.gl_marker(u, 0.0);
        let shown = scene.terrain.shown().is_some();
        let top = m[2].max(if shown { m[3] } else { 0.0 }) as f64;
        let point = (m[0] as f64 + ox, m[1] as f64 + oy, top);
        let depth = scene.view_at(0.0, u).view(point.0, point.1, point.2).depth;
        let near = scene.view.fitted().focal() * 2f64.powf(scene.view.fitted().zoom - zoom);
        (point, depth, near)
    }

    #[test]
    fn following_keeps_the_marker_at_the_zooms_depth() {
        // No terrain: the track stands at its exaggerated lift.
        let mut scene = scene();
        scene.set_follow(Some(scene.view.fitted().zoom + 3.0));
        for u in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let (point, depth, near) = marker_depth(&scene, u);
            assert!(
                (depth - near).abs() < 1e-6 * near,
                "{depth} vs {near} at {u}"
            );
            let camera = scene.view_at(0.0, u);
            let (x, y) = camera
                .project_world(point.0, point.1, point.2)
                .expect("in front");
            let (w, h) = camera.viewport();
            assert!(
                (x - w / 2.0).abs() < 1e-3 && (y - h / 2.0).abs() < 1e-3,
                "{x}, {y}"
            );
        }
    }

    #[test]
    fn following_over_terrain_keeps_the_eye_above_it() {
        let mut scene = with_terrain();
        scene.set_follow(Some(scene.view.fitted().zoom + 3.0));
        let Some(terrain) = scene.terrain.built() else {
            panic!("terrain is set");
        };
        let (ox, oy) = scene.gl_origin();
        let mut at_zoom = 0;
        for k in 0..=10 {
            let u = k as f64 / 10.0;
            let (point, depth, near) = marker_depth(&scene, u);
            let c = scene.gl_camera(0.0, u);
            let ground = terrain.height_at_world(c[0] + ox, c[1] + oy);
            let clearance = near * EYE_CLEARANCE;
            assert!(
                c[2] >= ground + clearance - 1e-6,
                "{} vs {ground} at {u}",
                c[2]
            );
            // Backing off only ever moves the marker farther.
            assert!(depth >= near * (1.0 - 1e-6), "{depth} vs {near}");
            if (depth - near).abs() < 1e-6 * near {
                at_zoom += 1;
            }
            let camera = scene.view_at(0.0, u);
            let (x, y) = camera
                .project_world(point.0, point.1, point.2)
                .expect("in front");
            let (w, h) = camera.viewport();
            assert!(
                (x - w / 2.0).abs() < 1.0 && (y - h / 2.0).abs() < 1.0,
                "{x}, {y}"
            );
        }
        assert!(
            at_zoom > 0,
            "the asked-for zoom holds where the ground allows"
        );
    }

    #[test]
    fn pitch_and_framing_zoom_move_only_the_camera() {
        let mut scene = scene();
        let origin = scene.gl_origin();
        let track = scene.gl_track();
        let zoom = scene.view.fitted().zoom;
        let fitted = scene.gl_camera(0.2, 0.0);
        scene.set_pitch(20.0);
        assert_eq!(scene.pitch_deg(), 20.0);
        assert_eq!(scene.view.fitted().zoom, zoom, "same world zoom");
        assert_eq!(scene.gl_origin(), origin);
        assert_eq!(scene.gl_track(), track);
        assert_ne!(scene.gl_camera(0.2, 0.0), fitted);
        scene.set_pitch(90.0);
        assert_eq!(scene.pitch_deg(), 75.0);
        scene.set_pitch(f64::NAN);
        assert_eq!(scene.pitch_deg(), 75.0);

        // Back at the start's pitch, the same view.
        scene.set_pitch(55.0);
        let back = scene.gl_camera(0.2, 0.0);
        assert!(
            (back[6] - fitted[6]).abs() < 1e-6 * fitted[6],
            "{} vs {}",
            back[6],
            fitted[6]
        );
        let base = back[6];
        scene.set_framing_zoom(2.0);
        let close = scene.gl_camera(0.2, 0.0)[6];
        assert!(
            (close - base / 4.0).abs() < 1e-6 * base,
            "{close} vs {base}"
        );
        scene.set_framing_zoom(10.0);
        assert_eq!(scene.framing_zoom(), 4.0);
        scene.set_framing_zoom(-5.0);
        assert_eq!(scene.framing_zoom(), -1.0);
        assert_eq!(scene.gl_origin(), origin);
    }

    #[test]
    fn framing_closer_over_terrain_clears_only_the_ground_under_it() {
        let mut scene = with_terrain();
        let far = scene.gl_camera(0.0, 0.0)[6];
        scene.set_framing_zoom(3.0);
        let Some(terrain) = scene.terrain.built() else {
            panic!("terrain is set");
        };
        let (ox, oy) = scene.gl_origin();
        let c = scene.gl_camera(0.0, 0.0);
        assert!(c[6] < far, "comes in: {} vs {far}", c[6]);
        let ground = terrain.height_at_world(c[0] + ox, c[1] + oy);
        assert!(c[2] > ground, "{} vs {ground}", c[2]);
    }

    #[test]
    fn framing_zoom_moves_the_view_smoothly() {
        for pitch in [55.0, 75.0] {
            let mut scene = scene();
            scene.set_pitch(pitch);
            for turn in [0.0, 0.3] {
                let mut last = scene.gl_camera(turn, 0.0);
                for k in 1..=80 {
                    scene.set_framing_zoom(k as f64 * 0.05);
                    let c = scene.gl_camera(turn, 0.0);
                    // The target stays put; only the distance shrinks.
                    let moved = (c[3] - last[3]).hypot(c[4] - last[4]);
                    assert!(moved < 1e-6, "pitch {pitch} turn {turn}: {moved}");
                    assert!(c[6] < last[6]);
                    last = c;
                }
                scene.set_framing_zoom(0.0);
            }
        }
    }

    #[test]
    fn framing_over_terrain_comes_in_without_a_jump() {
        for pitch in [20.0, 55.0, 75.0] {
            let mut scene = with_terrain();
            scene.set_pitch(pitch);
            let at_fit = scene.gl_camera(0.0, 0.0)[6];
            scene.set_framing_zoom(1e-6);
            let just_in = scene.gl_camera(0.0, 0.0)[6];
            assert!(
                (just_in - at_fit).abs() < 1e-3 * at_fit,
                "pitch {pitch}: {just_in} vs {at_fit}"
            );
        }
    }

    #[test]
    fn far_tiles_take_every_few_terrain_lines() {
        let lines: Vec<f64> = (0..=1000).map(|i| i as f64).collect();
        let near = grid_lines(100.0, 50.0, 0.0, &lines);
        assert_eq!(near.len(), 101, "every line and midway line in 50 px");
        let far = grid_lines(0.0, 1000.0, 0.0, &lines);
        assert!(far.len() <= 2 * MAX_TILE_LINES + 3, "{}", far.len());
        assert_eq!((far[0], far[far.len() - 1]), (0.0, 1000.0));
    }

    #[test]
    fn ground_meshes_follow_the_terrain_and_lift_the_camera() {
        let flat = scene();
        let scene = with_terrain();
        let tree = scene.gl_ground(0.2, 0.0, 2.0);
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
            let (nx, ny) = (mesh[3] as usize, mesh[4] as usize);
            let points = (nx + 1) * (ny + 1);
            assert_eq!(mesh.len(), 5 + nx + ny + 2 + 2 * points);
            let heights = &mesh[7 + nx + ny..7 + nx + ny + points];
            top = heights.iter().copied().fold(top, f32::max);
            if nx > 1 {
                raised += 1;
            }
            let flat_mesh = flat.gl_ground_mesh(t[0], t[1], t[2]);
            assert_eq!(flat_mesh[3..5], [1.0, 1.0], "no terrain, no relief");
        }
        assert!(raised > 0 && top > 0.0);
        let track = scene.gl_track();
        assert!(
            track.chunks(TRACK_STRIDE).any(|v| v[3] > 0.0),
            "stands on the slope"
        );
        for turn in [0.0, 0.5] {
            let eye_h = scene.gl_camera(turn, 0.0)[2];
            assert!(
                eye_h >= top as f64 * TERRAIN_CLEARANCE - 1e-3,
                "turn {turn}"
            );
        }
        let mut hidden = with_terrain();
        hidden.set_terrain_visible(false);
        assert_eq!(
            hidden.gl_camera(0.3, 0.0)[..6],
            flat_camera(&hidden, 0.3)[..6]
        );
    }

    #[test]
    fn grid_lines_take_in_the_terrain_lines_and_midpoints() {
        // Terrain cells of 10 from 100 to 140; a tile from 95 to 127.
        let cells = [0.0, 10.0, 20.0, 30.0, 40.0];
        let lines = grid_lines(95.0, 32.0, 100.0, &cells);
        assert_eq!(lines, vec![0.0, 5.0, 10.0, 15.0, 20.0, 25.0, 30.0, 32.0]);
        assert_eq!(grid_lines(0.0, 32.0, 100.0, &cells), vec![0.0, 32.0]);
        assert_eq!(grid_lines(200.0, 32.0, 100.0, &cells), vec![0.0, 32.0]);
        // A tile inside one half cell has just its ends.
        assert_eq!(grid_lines(101.0, 2.0, 100.0, &cells), vec![0.0, 2.0]);
        // Uneven cells halve too.
        let lines = grid_lines(100.0, 40.0, 100.0, &[0.0, 4.0, 40.0]);
        assert_eq!(lines, vec![0.0, 2.0, 4.0, 22.0, 40.0]);
    }

    #[test]
    fn ground_meshes_lie_on_the_terrain() {
        let scene = with_terrain();
        let Some(terrain) = scene.terrain.built() else {
            panic!("terrain is built");
        };
        let (ox, oy) = scene.gl_origin();
        let tree = scene.gl_ground(0.2, 0.0, 2.0);
        for t in tree.chunks(GROUND_STRIDE).filter(|t| t[4] == 0) {
            let mesh = scene.gl_ground_mesh(t[0], t[1], t[2]);
            let (nx, ny) = (mesh[3] as usize, mesh[4] as usize);
            let xs = &mesh[5..6 + nx];
            let ys = &mesh[6 + nx..7 + nx + ny];
            let h = &mesh[7 + nx + ny..];
            let world = |dx: f32, dy: f32| {
                (
                    mesh[0] as f64 + ox + dx as f64,
                    mesh[1] as f64 + oy + dy as f64,
                )
            };
            // Midway through each cell's two triangles the mesh agrees with
            // the terrain, away from the clipped cells at the tile's edges.
            for r in 1..ny.saturating_sub(1) {
                for c in 1..nx.saturating_sub(1) {
                    let at = |c: usize, r: usize| h[r * (nx + 1) + c] as f64;
                    for (fx, fy) in [(0.7, 0.2), (0.2, 0.7)] {
                        let px = xs[c] + (xs[c + 1] - xs[c]) * fx;
                        let py = ys[r] + (ys[r + 1] - ys[r]) * fy;
                        let mesh_h = if fx >= fy {
                            at(c, r)
                                + fx as f64 * (at(c + 1, r) - at(c, r))
                                + fy as f64 * (at(c + 1, r + 1) - at(c + 1, r))
                        } else {
                            at(c, r)
                                + fy as f64 * (at(c, r + 1) - at(c, r))
                                + fx as f64 * (at(c + 1, r + 1) - at(c, r + 1))
                        };
                        let (wx, wy) = world(px, py);
                        let ground = terrain.height_at_world(wx, wy);
                        assert!((mesh_h - ground).abs() < 0.05, "{mesh_h} vs {ground}");
                    }
                }
            }
        }
    }

    fn flat_camera(scene: &SceneRenderer, turn: f64) -> Vec<f64> {
        let camera = scene.view.camera_at(&scene.route, turn);
        let (ox, oy) = scene.gl_origin();
        let (ex, ey, eh) = camera.eye();
        let (tx, ty) = camera.target();
        vec![ex - ox, ey - oy, eh, tx - ox, ty - oy, camera.fov_deg()]
    }

    #[test]
    fn profile_strip_has_the_panel_and_clear_margins() {
        let scene = scene();
        let strip = scene.gl_profile(0.5);
        let (w, h) = (480usize, scene.gl_profile_height() as usize);
        assert_eq!(strip.len(), w * h * 4);
        assert_eq!(strip[3], 0, "the corner stays clear");
        let middle = ((h / 2) * w + w / 2) * 4;
        assert!(strip[middle + 3] > 0, "the panel is drawn");
    }
}
