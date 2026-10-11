//! Ground relief for the 3D view: DEM tiles plus a map texture around the
//! route.

use std::error::Error;

use gpx_motion_core::{Dem, Relief, Terrain, TerrainArea, TilePlacement, TrackPoint};
use tiny_skia::{Color, Pixmap};

use crate::tiles::{MapSource, TileFetcher};

/// Largest terrain texture side in pixels.
const MAX_TEXTURE: f64 = 4096.0;

/// What the terrain texture is painted on: the frame background and the map
/// opacity over it.
#[derive(Debug, Clone, Copy)]
pub struct Backdrop {
    pub color: Color,
    pub map_opacity: f32,
}

/// Fetch Terrarium tiles from `template` and drape the map over them around
/// `points`, at the camera's `zoom`.
///
/// The texture is the 2D map (or the backdrop color without one) at the
/// backdrop's opacity. DEM tiles span the texture plus one sample on each
/// side for bilinear lookups at the border.
///
/// Returns `None` with a warning when the area would need a texture larger
/// than [`MAX_TEXTURE`] (e.g. a long route at a fixed close zoom).
pub fn build(
    template: &str,
    cache: Option<std::path::PathBuf>,
    points: &[TrackPoint],
    relief: &Relief,
    zoom: f64,
    map: Option<&mut MapSource>,
    backdrop: Backdrop,
) -> Result<Option<Terrain>, Box<dyn Error>> {
    let Backdrop {
        color: background,
        map_opacity: opacity,
    } = backdrop;
    let Some(area) = TerrainArea::around(points, zoom, MAX_TEXTURE) else {
        return Ok(None);
    };
    if !area.fits(MAX_TEXTURE) {
        eprintln!("warning: route too large for terrain at zoom {zoom:.1}; drawing a flat map");
        return Ok(None);
    }
    let mut texture = Pixmap::new(area.width, area.height).ok_or("invalid terrain size")?;
    texture.fill(background);
    if let Some(map) = map {
        map.draw(&mut texture, &area.projection(), opacity);
    }

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
    let mut fetcher = TileFetcher::new(template.to_string(), cache)?;
    let mut dem = Dem::new(area.dem_zoom);
    for (placement, pixmap) in fetcher.fetch_all(&placements) {
        if !dem.add_terrarium(placement.id, pixmap.data()) {
            eprintln!(
                "warning: terrain tile {}/{}/{} is not a 256px Terrarium tile",
                placement.id.zoom, placement.id.x, placement.id.y
            );
        }
    }
    Ok(Some(Terrain::build(
        &dem,
        relief,
        zoom,
        area.origin,
        texture,
    )))
}
