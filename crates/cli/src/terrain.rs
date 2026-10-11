//! Ground relief for the 3D view: DEM tiles plus a map texture around the
//! route.

use std::error::Error;
use std::path::PathBuf;

use gpx_motion_core::{Dem, Relief, Surroundings, Terrain, TerrainArea, TilePlacement, TrackPoint};
use tiny_skia::{Color, Pixmap};

use crate::tiles::{MapSource, TileFetcher};

/// Largest terrain texture side in pixels.
const MAX_TEXTURE: f64 = 4096.0;
/// Margin around the route covered by the coarse surroundings of a view
/// that zooms out, as a multiple of the route's larger extent per side.
const WIDE_MARGIN: f64 = 1.0;
/// Zoom levels the widest view must sit below the camera's for coarse
/// surroundings to be worth building.
const MIN_ZOOM_OUT: f64 = 0.5;

/// What the terrain texture is painted on: the frame background and the map
/// opacity over it.
#[derive(Debug, Clone, Copy)]
pub struct Backdrop {
    pub color: Color,
    pub map_opacity: f32,
}

/// Where the terrain's elevation and map come from.
pub struct Sources<'a> {
    /// Terrarium tile URL template.
    pub template: &'a str,
    pub cache: Option<PathBuf>,
    pub map: Option<&'a mut MapSource>,
    pub backdrop: Backdrop,
}

/// Fetch Terrarium tiles and drape the map over them around `points`, at
/// the camera's `zoom`.
///
/// The texture is the 2D map (or the backdrop color without one) at the
/// backdrop's opacity. DEM tiles span the texture plus one sample on each
/// side for bilinear lookups at the border.
///
/// With `wide_zoom` (the detail zoom of the widest view, e.g. zoomed out at
/// the ends of a video), a wider area around the route is added at that
/// zoom's coarser map and DEM, so the relief continues as far as that view
/// sees.
///
/// Returns `None` with a warning when the area would need a texture larger
/// than [`MAX_TEXTURE`] (e.g. a long route at a fixed close zoom).
pub fn build(
    sources: Sources,
    points: &[TrackPoint],
    relief: &Relief,
    zoom: f64,
    wide_zoom: Option<f64>,
) -> Result<Option<Terrain>, Box<dyn Error>> {
    let Sources {
        template,
        cache,
        mut map,
        backdrop,
    } = sources;
    let Some(area) = TerrainArea::around(points, zoom, MAX_TEXTURE) else {
        return Ok(None);
    };
    if !area.fits(MAX_TEXTURE) {
        eprintln!("warning: route too large for terrain at zoom {zoom:.1}; drawing a flat map");
        return Ok(None);
    }
    let mut fetcher = TileFetcher::new(template.to_string(), cache)?;
    let texture = map_texture(&area, map.as_deref_mut(), backdrop)?;
    let dem = fetch_dem(&mut fetcher, &area);
    let wide = wide_zoom
        .filter(|&z| z < zoom - MIN_ZOOM_OUT)
        .and_then(|z| TerrainArea::around_with_margin(points, z, WIDE_MARGIN, MAX_TEXTURE))
        .filter(|wide| wide.fits(MAX_TEXTURE));
    let Some(wide) = wide else {
        return Ok(Some(Terrain::build(
            &dem,
            relief,
            zoom,
            area.origin,
            texture,
        )));
    };
    let wide_dem = fetch_dem(&mut fetcher, &wide);
    let scale = 2f64.powf(zoom - wide.zoom);
    let around = Surroundings {
        dem: &wide_dem,
        origin: (wide.origin.0 * scale, wide.origin.1 * scale),
        size: (wide.width as f64 * scale, wide.height as f64 * scale),
        map: map_texture(&wide, map, backdrop)?,
    };
    Ok(Some(Terrain::with_surroundings(
        &dem,
        relief,
        zoom,
        area.origin,
        texture,
        around,
    )))
}

/// The map over `area` (one texel per world pixel at its zoom) on the
/// backdrop.
fn map_texture(
    area: &TerrainArea,
    map: Option<&mut MapSource>,
    backdrop: Backdrop,
) -> Result<Pixmap, Box<dyn Error>> {
    let mut texture = Pixmap::new(area.width, area.height).ok_or("invalid terrain size")?;
    texture.fill(backdrop.color);
    if let Some(map) = map {
        map.draw(&mut texture, &area.projection(), backdrop.map_opacity);
    }
    Ok(texture)
}

/// Elevation over `area` from Terrarium tiles.
fn fetch_dem(fetcher: &mut TileFetcher, area: &TerrainArea) -> Dem {
    let placements: Vec<TilePlacement> = area
        .dem_tiles()
        .into_iter()
        .map(|id| TilePlacement {
            id,
            dx: 0.0,
            dy: 0.0,
            size: 256.0,
        })
        .collect();
    let mut dem = Dem::new(area.dem_zoom);
    for (placement, pixmap) in fetcher.fetch_all(&placements) {
        if !dem.add_terrarium(placement.id, pixmap.data()) {
            eprintln!(
                "warning: terrain tile {}/{}/{} is not a 256px Terrarium tile",
                placement.id.zoom, placement.id.x, placement.id.y
            );
        }
    }
    dem
}
