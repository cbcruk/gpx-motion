pub mod fetcher;
pub mod pmtiles;
pub mod vector;

pub use fetcher::TileFetcher;
pub use vector::VectorTileFetcher;

use anyhow::{bail, Result};
use gpx_motion_core::geo::mercator::{tile_levels, tile_zoom_for, Projection, TileLevels};
use std::collections::HashMap;

use gpx_motion_core::{
    compose, draw_vector_map, draw_vector_map_3d, extrude, place_labels_flat, Buildings, Camera,
    Keepout, LabelMemory, LabelSources, LabelStyle, MapStyle, PlacedLabel, Terrain, TileId,
    TilePlacement, VectorTile,
};
use tiny_skia::{Color, Pixmap};

/// Where the background map comes from.
pub enum MapSource {
    /// Pre-rendered image tiles (TMS/XYZ).
    Raster(TileFetcher),
    /// MVT tiles rendered locally with a label-free style.
    Vector {
        tiles: VectorTileFetcher,
        style: MapStyle,
    },
}

impl MapSource {
    /// Draw the map covering `proj`'s viewport onto `target` at `opacity`;
    /// with `fade` (a zooming view), near a tile zoom switch the next zoom
    /// fades in (see [`tile_levels`]).
    pub fn draw(&mut self, target: &mut Pixmap, proj: &Projection, opacity: f32, fade: bool) {
        let (width, height) = (target.width(), target.height());
        match self {
            MapSource::Raster(fetcher) => {
                let tiles = fetcher.fetch_all(&proj.visible_tiles(width, height));
                compose(target, &tiles, opacity);
            }
            MapSource::Vector { tiles, style } => {
                draw_flat(target, tiles, style, proj, opacity, fade);
            }
        }
    }

    /// Draw the map covering `proj` as ground a 3D view stands buildings
    /// on: without the extrusions it raises instead (see
    /// [`MapSource::buildings`]).
    pub fn draw_ground(&mut self, target: &mut Pixmap, proj: &Projection, opacity: f32) {
        match self {
            MapSource::Vector { tiles, style } if style.extrudes() => {
                draw_flat(
                    target,
                    tiles,
                    &style.without_extrusions(),
                    proj,
                    opacity,
                    false,
                );
            }
            _ => self.draw(target, proj, opacity, false),
        }
    }

    /// The buildings the style raises under `camera`, from the prefetched
    /// tiles (the more opaque zoom while fading, as with `fade` in
    /// [`MapSource::draw_3d`]).
    pub fn buildings(&self, camera: &Camera, fade: bool) -> Vec<Buildings> {
        let MapSource::Vector { tiles, style } = self else {
            return Vec::new();
        };
        if !style.extrudes() {
            return Vec::new();
        }
        let placements = ground_tile_sets(tiles, style, camera, fade).shown();
        extrude(&tiles.cached(&placements), style, camera.detail_zoom)
    }

    /// Fetch the vector tiles `camera` will see, so later draws can share the
    /// source read-only (e.g. across render threads).
    ///
    /// Raster tiles can't be drawn in perspective, so they are an error here
    /// ([`Config`](crate::config::Config) rejects them before any I/O).
    /// `fade` as in [`MapSource::draw_3d`].
    pub fn prefetch_3d(&mut self, camera: &Camera, fade: bool) -> Result<()> {
        let MapSource::Vector { tiles, style } = self else {
            bail!("the 3D view needs --vector-tiles; raster tiles can't be tilted");
        };
        ground_tile_sets(tiles, style, camera, fade).prefetch(tiles);
        if let Some(reason) = tiles.failure() {
            bail!("{reason}");
        }
        Ok(())
    }

    /// Why the source stopped loading tiles mid-run, if it did (see
    /// [`VectorTileFetcher::failure`]).
    pub fn failure(&self) -> Option<&str> {
        match self {
            MapSource::Vector { tiles, .. } => tiles.failure(),
            MapSource::Raster(_) => None,
        }
    }

    /// Draw the prefetched ground under `camera` (no fog; see
    /// [`MapSource::fog_color`]); with `fade` (a zooming view), near a tile
    /// zoom switch the next zoom fades in.
    pub fn draw_3d(&self, target: &mut Pixmap, camera: &Camera, opacity: f32, fade: bool) {
        if let MapSource::Vector { tiles, style } = self {
            let sets = ground_tile_sets(tiles, style, camera, fade).cached(tiles);
            draw_vector_map_3d(target, &sets.base, sets.finer(), style, camera, opacity);
        }
    }

    /// Labels for the vector map under `proj` (none for raster maps), from
    /// the tiles [`MapSource::draw`] drew with the same
    /// [`LabelContext::fade`].
    pub fn labels_2d(&self, proj: &Projection, context: &LabelContext) -> Vec<PlacedLabel> {
        let MapSource::Vector { tiles, style } = self else {
            return Vec::new();
        };
        let placements = flat_tiles(tiles, style, proj, context.canvas, context.fade).shown();
        let decoded = tiles.cached(&placements);
        place_labels_flat(&decoded, context.style, context.canvas, context.keepout)
    }

    /// Labels for the prefetched vector ground under `camera`, smaller with
    /// distance and dropped toward the horizon fog; over `terrain` they
    /// stand on the ground and the hills hide some. `memory` carries labels
    /// from the previous frame (see [`LabelMemory`]); `cache` keeps each
    /// tile's label sources between frames.
    pub fn labels_3d(
        &self,
        camera: &Camera,
        context: &LabelContext,
        terrain: Option<&Terrain>,
        memory: &mut LabelMemory,
        cache: &mut LabelCache,
    ) -> Vec<PlacedLabel> {
        let MapSource::Vector { tiles, style } = self else {
            return Vec::new();
        };
        let placements = ground_tile_sets(tiles, style, camera, context.fade).shown();
        let decoded = tiles.cached(&placements);
        for &(placement, tile) in &decoded {
            cache
                .entry(placement_key(&placement))
                .or_insert_with(|| LabelSources::collect(&[(placement, tile)], context.style));
        }
        let parts: Vec<&LabelSources> = decoded
            .iter()
            .filter_map(|(p, _)| cache.get(&placement_key(p)))
            .collect();
        LabelSources::place_3d_steady_parts(
            &parts,
            camera,
            context.style,
            context.canvas,
            context.keepout,
            terrain,
            memory,
        )
    }

    /// Color the far ground fades into: the vector style's background, so the
    /// edge of the fetched tiles never shows.
    pub fn fog_color(&self) -> Option<Color> {
        match self {
            MapSource::Vector { style, .. } => Some(style.background),
            MapSource::Raster(_) => None,
        }
    }
}

/// How one frame's map labels are placed, shared by
/// [`MapSource::labels_2d`] and [`MapSource::labels_3d`].
#[derive(Clone, Copy)]
pub struct LabelContext<'a> {
    /// Frame size in device pixels.
    pub canvas: (u32, u32),
    pub style: &'a LabelStyle,
    /// What the labels stay clear of.
    pub keepout: &'a Keepout,
    /// Whether the view zooms, so labels come from the more opaque of two
    /// fading tile zooms (as the map draws them).
    pub fade: bool,
}

/// The deepest tile zoom to draw world `zoom` from: the source's limit, and
/// for styles imported from MapLibre the data zoom MapLibre would use.
fn tile_cap(tiles: &VectorTileFetcher, style: &MapStyle, zoom: f64) -> u32 {
    style
        .data_zoom(zoom)
        .map_or(tiles.max_zoom, |z| z.min(tiles.max_zoom))
}

/// The tile zooms to draw world `zoom` from: [`tile_cap`] below the
/// zoom's own tile zoom, fading into the next near a switch.
fn levels(tiles: &VectorTileFetcher, style: &MapStyle, zoom: f64) -> TileLevels {
    tile_levels(zoom, |z| tile_cap(tiles, style, z).min(tile_zoom_for(z)))
}

/// Tiles one view draws: one zoom's, and near a switch the next zoom's
/// fading in with their opacity.
struct TileSets {
    base: Vec<TilePlacement>,
    finer: Option<(Vec<TilePlacement>, f32)>,
}

impl TileSets {
    /// The sets from tile zooms `levels`, each placed by `place`; a finer
    /// zoom placed no finer than the base (a tile budget dropping it) is
    /// left out.
    fn of(levels: TileLevels, place: impl Fn(u32) -> Vec<TilePlacement>) -> Self {
        let base = place(levels.base);
        let finer = levels.finer.map(|(zoom, mix)| (place(zoom), mix));
        let zoom = |set: &[TilePlacement]| set.first().map(|p| p.id.zoom);
        let finer = finer.filter(|(set, _)| zoom(set) > zoom(&base));
        Self { base, finer }
    }

    fn prefetch(&self, tiles: &mut VectorTileFetcher) {
        tiles.prefetch(&self.base);
        if let Some((finer, _)) = &self.finer {
            tiles.prefetch(finer);
        }
    }

    /// The decoded tiles of each set (those already fetched).
    fn cached<'a>(&self, tiles: &'a VectorTileFetcher) -> DecodedSets<'a> {
        DecodedSets {
            base: tiles.cached(&self.base),
            finer: self
                .finer
                .as_ref()
                .map(|(set, mix)| (tiles.cached(set), *mix)),
        }
    }

    /// The set that shows the most: the finer one once it is half faded in.
    fn shown(self) -> Vec<TilePlacement> {
        match self.finer {
            Some((finer, mix)) if mix >= 0.5 => finer,
            _ => self.base,
        }
    }
}

/// Decoded tiles placed on screen.
type Decoded<'a> = Vec<(TilePlacement, &'a VectorTile)>;

/// [`TileSets`] with their tiles decoded, ready to draw.
struct DecodedSets<'a> {
    base: Decoded<'a>,
    finer: Option<(Decoded<'a>, f32)>,
}

impl DecodedSets<'_> {
    /// The finer set and its opacity, as the map drawers take it.
    fn finer(&self) -> Option<(&[(TilePlacement, &VectorTile)], f32)> {
        self.finer
            .as_ref()
            .map(|(decoded, mix)| (&decoded[..], *mix))
    }
}

/// Draw vector `tiles` covering `proj`'s viewport onto `target` in `style`,
/// as [`MapSource::draw`] does.
fn draw_flat(
    target: &mut Pixmap,
    tiles: &mut VectorTileFetcher,
    style: &MapStyle,
    proj: &Projection,
    opacity: f32,
    fade: bool,
) {
    let canvas = (target.width(), target.height());
    let sets = flat_tiles(tiles, style, proj, canvas, fade);
    sets.prefetch(tiles);
    let decoded = sets.cached(tiles);
    draw_vector_map(target, &decoded.base, decoded.finer(), style, opacity);
}

/// Tiles over `proj`'s `canvas`, fading between zooms with `fade`.
fn flat_tiles(
    tiles: &VectorTileFetcher,
    style: &MapStyle,
    proj: &Projection,
    (width, height): (u32, u32),
    fade: bool,
) -> TileSets {
    if !fade {
        let cap = tile_cap(tiles, style, proj.zoom);
        return TileSets {
            base: proj.visible_tiles_at(cap, width, height),
            finer: None,
        };
    }
    TileSets::of(levels(tiles, style, proj.zoom), |zoom| {
        proj.tiles_of_zoom(zoom, width, height)
    })
}

/// Ground tiles under `camera`, fading between zooms with `fade`.
///
/// The tile budget drops far views to coarser zooms as the camera backs
/// off, not as its detail zoom changes, so a fading view picks its zoom
/// from the lower of the detail zoom and [`Camera::ground_budget_zoom`],
/// which both move smoothly.
fn ground_tile_sets(
    tiles: &VectorTileFetcher,
    style: &MapStyle,
    camera: &Camera,
    fade: bool,
) -> TileSets {
    if !fade {
        let cap = tile_cap(tiles, style, camera.detail_zoom);
        return TileSets {
            base: camera.ground_tiles(cap),
            finer: None,
        };
    }
    let zoom = camera.detail_zoom.min(camera.ground_budget_zoom());
    TileSets::of(levels(tiles, style, zoom), |zoom| {
        camera.ground_tiles_of_zoom(zoom)
    })
}

/// Label sources of single tiles, kept across frames by [`placement_key`].
pub type LabelCache = HashMap<(TileId, u64, u64), LabelSources>;

/// A tile placement as a cache key (the tile and where it sits).
fn placement_key(p: &TilePlacement) -> (TileId, u64, u64) {
    (p.id, p.dx.to_bits(), p.dy.to_bits())
}
