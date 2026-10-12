//! The ground relief under the scene: the terrain area chosen when the
//! scene is built, the DEM tiles the page fetches for it, and the terrain
//! built from them.

use std::fmt;

use gpx_motion_core::{Dem, Relief, Terrain, TerrainArea, TilePlacement, TrackPoint};
use wasm_bindgen::prelude::*;

use super::SceneRenderer;
use crate::Tile;

/// Largest terrain area side in world pixels.
const MAX_TERRAIN_SIDE: f64 = 2048.0;
/// Most terrain grid cells along the area's longer side, finer than the CPU
/// renderer's for a camera following the marker closer than the area's own
/// zoom (cells stay at least 12 area pixels, so a long route seen at a much
/// finer zoom still shows broad relief).
const TERRAIN_CELLS: f64 = 256.0;

/// Why a scene can't have terrain. Every message starts with `no terrain:`,
/// which the page tells apart from other construction errors by.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum TerrainError {
    /// The track doesn't fit the viewport with its relief capped for
    /// terrain.
    Unfit,
    /// The area around the route, at the zoom that frames it, is larger
    /// than [`MAX_TERRAIN_SIDE`] on a side.
    TooLarge { width: u32, height: u32 },
}

impl fmt::Display for TerrainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unfit => f.write_str(
                "no terrain: the track does not fit the viewport with its relief capped for terrain",
            ),
            Self::TooLarge { width, height } => write!(
                f,
                "no terrain: the route is too large for a terrain area \
                 ({width}×{height} px at its zoom, over {MAX_TERRAIN_SIDE} px)",
            ),
        }
    }
}

/// The scene's terrain: none on flat ground, else the area it covers, the
/// DEM tiles added for it, and the terrain once built from them.
#[derive(Default)]
pub(super) struct TerrainState {
    area: Option<TerrainArea>,
    dem: Option<Dem>,
    built: Option<Terrain>,
    hidden: bool,
}

impl TerrainState {
    /// Flat ground: no terrain to list tiles for or build.
    pub fn flat() -> Self {
        Self::default()
    }

    /// Terrain around `points` at world zoom `zoom`, waiting for its DEM
    /// tiles.
    pub fn around(points: &[TrackPoint], zoom: f64) -> Result<Self, TerrainError> {
        // `None` only without points, and a scene's track has a timeline,
        // so at least two.
        let area = TerrainArea::around(points, zoom, MAX_TERRAIN_SIDE)
            .expect("a scene's track has points");
        if !area.fits(MAX_TERRAIN_SIDE) {
            return Err(TerrainError::TooLarge {
                width: area.width,
                height: area.height,
            });
        }
        Ok(Self {
            area: Some(area),
            dem: Some(Dem::new(area.dem_zoom)),
            ..Self::default()
        })
    }

    /// The area the terrain covers, `None` on flat ground.
    pub fn area(&self) -> Option<TerrainArea> {
        self.area
    }

    /// The terrain, once built.
    pub fn built(&self) -> Option<&Terrain> {
        self.built.as_ref()
    }

    /// The terrain the scene stands on, when built and not hidden.
    pub fn shown(&self) -> Option<&Terrain> {
        self.built.as_ref().filter(|_| !self.hidden)
    }

    /// The terrain's top-left corner in world pixels: the built terrain's,
    /// else the area's; `None` on flat ground.
    pub fn origin(&self) -> Option<(f64, f64)> {
        self.built
            .as_ref()
            .map(Terrain::origin)
            .or(self.area.map(|a| a.origin))
    }
}

#[wasm_bindgen]
impl SceneRenderer {
    /// Terrarium DEM tiles (XYZ PNGs, e.g. the AWS Terrain Tiles) the terrain
    /// needs, for a scene built with terrain; empty on flat ground.
    #[wasm_bindgen(js_name = terrainTiles)]
    pub fn terrain_tiles(&self) -> Vec<Tile> {
        self.terrain.area.map_or_else(Vec::new, |area| {
            area.dem_tiles()
                .into_iter()
                .map(|id| Tile::from(TilePlacement::whole(id)))
                .collect()
        })
    }

    /// Add a decoded 256×256 Terrarium tile (RGBA bytes) listed by
    /// [`SceneRenderer::terrain_tiles`]. Returns whether it was used.
    #[wasm_bindgen(js_name = addTerrainTile)]
    pub fn add_terrain_tile(&mut self, tile: &Tile, rgba: &[u8]) -> bool {
        self.terrain
            .dem
            .as_mut()
            .is_some_and(|dem| dem.add_terrarium(TilePlacement::from(tile).id, rgba))
    }

    /// Build the terrain from the DEM tiles added so far, without a map
    /// texture (the WebGL scene drapes its own map tiles over it; see
    /// [`SceneRenderer::gl_ground_mesh`]). Errors on flat ground.
    ///
    /// # Errors
    ///
    /// Throws when the scene was built without terrain.
    #[wasm_bindgen(js_name = setTerrainHeights)]
    pub fn set_terrain_heights(&mut self) -> Result<(), JsError> {
        let terrain = &mut self.terrain;
        let (Some(area), Some(dem)) = (terrain.area, &terrain.dem) else {
            return Err(JsError::new("the scene was built without terrain"));
        };
        terrain.built = Some(build(dem, &self.route.relief, area));
        Ok(())
    }

    /// Show or hide the terrain (shown by default once built).
    #[wasm_bindgen(js_name = setTerrainVisible)]
    pub fn set_terrain_visible(&mut self, visible: bool) {
        self.terrain.hidden = !visible;
    }
}

/// The bare terrain of `area` from `dem`, raised by `relief`.
fn build(dem: &Dem, relief: &Relief, area: TerrainArea) -> Terrain {
    Terrain::bare_with_cells(
        dem,
        relief,
        area.zoom,
        area.origin,
        (area.width, area.height),
        TERRAIN_CELLS,
    )
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{namsan, with_terrain};
    use super::*;

    #[test]
    fn flat_ground_has_no_terrain_to_build() {
        let mut scene = namsan(false);
        assert!(scene.terrain.area().is_none() && scene.terrain.origin().is_none());
        assert!(scene.terrain_tiles().is_empty());
        let tile = Tile::new(12, 0, 0, 0.0, 0.0, 256.0);
        assert!(!scene.add_terrain_tile(&tile, &[0; 4]));
    }

    #[test]
    fn hiding_the_terrain_keeps_it_built() {
        let mut scene = with_terrain();
        let Some(area) = scene.terrain.area() else {
            panic!("terrain area");
        };
        assert_eq!(
            scene.terrain.origin(),
            scene.terrain.built().map(Terrain::origin)
        );
        assert!(scene.terrain.origin().is_some_and(|(x, y)| {
            (x - area.origin.0).abs() < 1.0 && (y - area.origin.1).abs() < 1.0
        }));
        scene.set_terrain_visible(false);
        assert!(scene.terrain.shown().is_none() && scene.terrain.built().is_some());
        scene.set_terrain_visible(true);
        assert!(scene.terrain.shown().is_some());
    }

    #[test]
    fn errors_name_their_reason() {
        let large = TerrainError::TooLarge {
            width: 4000,
            height: 300,
        };
        assert!(large.to_string().contains("too large"), "{large}");
        assert!(large.to_string().contains("4000×300"), "{large}");
        assert!(TerrainError::Unfit.to_string().contains("does not fit"));
        for e in [large, TerrainError::Unfit] {
            assert!(e.to_string().starts_with("no terrain: "), "{e}");
        }
    }
}
