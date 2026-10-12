//! Layouts of the flat number arrays handed to JS: which number of a record
//! holds which field, and how many numbers each record takes.
//!
//! The field enums are exported as JS enums and the strides as functions
//! (`trackStride()`, …), so the page reads these instead of repeating them.
//! The producers here write each record by these fields too.

use wasm_bindgen::prelude::*;

/// Fields of a track vertex in [`SceneRenderer::gl_track`](crate::SceneRenderer::gl_track).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackField {
    /// World `x` relative to the scene origin.
    X = 0,
    /// World `y` relative to the scene origin.
    Y = 1,
    /// Height of the relief over the ground.
    Lift = 2,
    /// Terrain height under the vertex (`0` without terrain).
    Ground = 3,
    /// Track progress (`0..=1`).
    U = 4,
    /// Segment color, red (`0..=1`).
    R = 5,
    /// Segment color, green (`0..=1`).
    G = 6,
    /// Segment color, blue (`0..=1`).
    B = 7,
}

/// Floats per vertex in [`SceneRenderer::gl_track`](crate::SceneRenderer::gl_track).
pub const TRACK_STRIDE: usize = TrackField::B as usize + 1;

/// Fields of the marker in [`SceneRenderer::gl_marker`](crate::SceneRenderer::gl_marker).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkerField {
    /// World `x`, as in [`TrackField::X`].
    X = 0,
    /// World `y`, as in [`TrackField::Y`].
    Y = 1,
    /// Relief height, as in [`TrackField::Lift`].
    Lift = 2,
    /// Terrain height, as in [`TrackField::Ground`].
    Ground = 3,
    /// `1` when the shown terrain hides the marker from the camera, else `0`.
    Hidden = 4,
}

/// Floats in [`SceneRenderer::gl_marker`](crate::SceneRenderer::gl_marker).
pub const MARKER_LEN: usize = MarkerField::Hidden as usize + 1;

/// Fields of the camera in [`SceneRenderer::gl_camera`](crate::SceneRenderer::gl_camera).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraField {
    /// Eye `x` relative to the scene origin.
    EyeX = 0,
    /// Eye `y` relative to the scene origin.
    EyeY = 1,
    /// Eye height.
    EyeH = 2,
    /// Target `x` (on the ground) relative to the scene origin.
    TargetX = 3,
    /// Target `y` (on the ground) relative to the scene origin.
    TargetY = 4,
    /// Vertical field of view in degrees.
    Fov = 5,
    /// Distance from the eye to the target.
    Distance = 6,
    /// Screen row of the fog band's horizon, `NaN` without fog.
    Horizon = 7,
    /// Screen row where the fog band ends, `NaN` without fog.
    FogEnd = 8,
    /// Bearing in degrees: the compass direction up the screen.
    Bearing = 9,
    /// The farthest view depth whose ground is drawn.
    Far = 10,
}

/// Numbers in [`SceneRenderer::gl_camera`](crate::SceneRenderer::gl_camera).
pub const CAMERA_LEN: usize = CameraField::Far as usize + 1;

/// Fields of a tile in [`SceneRenderer::gl_ground`](crate::SceneRenderer::gl_ground).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundNodeField {
    /// Tile zoom.
    Zoom = 0,
    /// Tile column.
    X = 1,
    /// Tile row.
    Y = 2,
    /// Index of the tile this one splits plus one (`0` for a root).
    Parent = 3,
    /// `1` when the tile is split further, else `0` (a leaf to draw).
    Split = 4,
}

/// Numbers per tile in [`SceneRenderer::gl_ground`](crate::SceneRenderer::gl_ground).
pub const GROUND_STRIDE: usize = GroundNodeField::Split as usize + 1;

/// Header fields of a tile mesh in
/// [`SceneRenderer::gl_ground_mesh`](crate::SceneRenderer::gl_ground_mesh).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroundMeshField {
    /// Top-left corner `x` relative to the scene origin.
    X = 0,
    /// Top-left corner `y` relative to the scene origin.
    Y = 1,
    /// Side in world pixels.
    Side = 2,
    /// Grid cells across (`nx`).
    CellsX = 3,
    /// Grid cells down (`ny`).
    CellsY = 4,
    /// Where the grid line offsets start, after the header: `nx + 1` across,
    /// `ny + 1` down, then heights and light per grid point.
    Lines = 5,
}

/// Fields of a building corner in
/// [`SceneRenderer::gl_buildings`](crate::SceneRenderer::gl_buildings) and
/// [`GroundTiles::buildings`](crate::GroundTiles::buildings).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildingField {
    /// Corner `x`.
    X = 0,
    /// Corner `y`.
    Y = 1,
    /// Height above the building's foot.
    Height = 2,
    /// Height of the building's foot in `glBuildings`; the corner's building
    /// index in `GroundTiles.buildings` (corners of one building come together).
    Foot = 3,
    /// Face color, red (`0..=1`, straight).
    R = 4,
    /// Face color, green.
    G = 5,
    /// Face color, blue.
    B = 6,
    /// Face opacity.
    A = 7,
}

/// Floats per corner in [`GroundTiles::buildings`](crate::GroundTiles::buildings)
/// and [`SceneRenderer::gl_buildings`](crate::SceneRenderer::gl_buildings).
pub const BUILDING_STRIDE: usize = BuildingField::A as usize + 1;

/// Fields of a label in [`GlLabels::labels`](crate::GlLabels::labels).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelField {
    /// Left end of the baseline, `x` (point labels).
    X = 0,
    /// Left end of the baseline, `y` (point labels).
    Y = 1,
    /// CSS font size.
    Size = 2,
    /// Opacity (`0..=1`).
    Opacity = 3,
    /// Halo width.
    Halo = 4,
    /// How far below a glyph's center its baseline sits (line labels).
    Rise = 5,
    /// The label's first glyph in [`GlLabels::glyphs`](crate::GlLabels::glyphs).
    First = 6,
    /// The label's glyph count (`0` for a point label).
    Count = 7,
    /// `1` for a line label in a shaped script, drawn whole along the line
    /// between its first and last glyph, else `0`.
    Whole = 8,
}

/// Floats per label in [`GlLabels::labels`](crate::GlLabels::labels).
pub const LABEL_STRIDE: usize = LabelField::Whole as usize + 1;

/// Fields of a glyph in [`GlLabels::glyphs`](crate::GlLabels::glyphs).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlyphField {
    /// Glyph center `x`.
    X = 0,
    /// Glyph center `y`.
    Y = 1,
    /// Angle in radians, clockwise from the x axis.
    Angle = 2,
    /// `1` to draw the glyph, `0` where something drawn over the label covers it.
    Shown = 3,
}

/// Floats per glyph in [`GlLabels::glyphs`](crate::GlLabels::glyphs).
pub const GLYPH_STRIDE: usize = GlyphField::Shown as usize + 1;

/// What [`PmTiles::locate`](crate::PmTiles::locate) asks for next.
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// The archive lacks the tile.
    Missing = 0,
    /// Fetch the byte range and pass it to [`PmTiles::tile`](crate::PmTiles::tile).
    Tile = 1,
    /// Fetch the leaf directory at the range, pass it to
    /// [`PmTiles::add_leaf`](crate::PmTiles::add_leaf) with the key, and ask again.
    Leaf = 2,
}

/// Fields of a step in [`PmTiles::locate`](crate::PmTiles::locate).
#[wasm_bindgen]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepField {
    /// The [`StepKind`].
    Kind = 0,
    /// Byte offset of the range to fetch (`0` when missing).
    Offset = 1,
    /// Byte length of the range to fetch (`0` when missing).
    Length = 2,
    /// The leaf directory's key (`0` unless a leaf).
    Key = 3,
}

/// Numbers in [`PmTiles::locate`](crate::PmTiles::locate).
pub const STEP_LEN: usize = StepField::Key as usize + 1;

/// Floats per vertex in `SceneRenderer.glTrack()` ([`TRACK_STRIDE`]).
#[wasm_bindgen(js_name = trackStride)]
pub fn track_stride() -> u32 {
    TRACK_STRIDE as u32
}

/// Numbers per tile in `SceneRenderer.glGround()` ([`GROUND_STRIDE`]).
#[wasm_bindgen(js_name = groundStride)]
pub fn ground_stride() -> u32 {
    GROUND_STRIDE as u32
}

/// Floats per corner in `SceneRenderer.glBuildings()` ([`BUILDING_STRIDE`]).
#[wasm_bindgen(js_name = buildingStride)]
pub fn building_stride() -> u32 {
    BUILDING_STRIDE as u32
}

/// Floats per label in `GlLabels.labels()` ([`LABEL_STRIDE`]).
#[wasm_bindgen(js_name = labelStride)]
pub fn label_stride() -> u32 {
    LABEL_STRIDE as u32
}

/// Floats per glyph in `GlLabels.glyphs()` ([`GLYPH_STRIDE`]).
#[wasm_bindgen(js_name = glyphStride)]
pub fn glyph_stride() -> u32 {
    GLYPH_STRIDE as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strides_match_the_records() {
        assert_eq!(TRACK_STRIDE, 8);
        assert_eq!(MARKER_LEN, 5);
        assert_eq!(CAMERA_LEN, 11);
        assert_eq!(GROUND_STRIDE, 5);
        assert_eq!(GroundMeshField::Lines as usize, 5);
        assert_eq!(BUILDING_STRIDE, 8);
        assert_eq!(LABEL_STRIDE, 9);
        assert_eq!(GLYPH_STRIDE, 4);
        assert_eq!(STEP_LEN, 4);
    }
}
