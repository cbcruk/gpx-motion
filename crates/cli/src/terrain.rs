//! Ground relief for the 3D view: DEM tiles plus a map texture around the
//! route.

use std::error::Error;

use gpx_motion_core::geo::mercator::{lat_to_pixel, lon_to_pixel, Projection};
use gpx_motion_core::{Dem, Relief, Terrain, TileId, TilePlacement, TrackPoint};
use tiny_skia::{Color, Pixmap};

use crate::tiles::{MapSource, TileFetcher};

/// Map texture around the route, as a multiple of the route's larger extent
/// added on each side.
const MARGIN: f64 = 0.6;
/// Largest terrain texture side in pixels.
const MAX_TEXTURE: f64 = 4096.0;
/// Highest zoom Terrarium tiles are published at.
const MAX_DEM_ZOOM: u32 = 15;

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
    zoom: u32,
    map: Option<&mut MapSource>,
    backdrop: Backdrop,
) -> Result<Option<Terrain>, Box<dyn Error>> {
    let Backdrop {
        color: background,
        map_opacity: opacity,
    } = backdrop;
    let Some((x0, y0, x1, y1)) = world_bounds(points, zoom) else {
        return Ok(None);
    };
    let extent = (x1 - x0).max(y1 - y0);
    let margin = (MARGIN * extent)
        .max(256.0)
        .min(((MAX_TEXTURE - extent) / 2.0).max(0.0));
    let (x0, y0) = ((x0 - margin).floor(), (y0 - margin).floor());
    let (w, h) = ((x1 + margin).ceil() - x0, (y1 + margin).ceil() - y0);
    if w > MAX_TEXTURE || h > MAX_TEXTURE {
        eprintln!("warning: route too large for terrain at zoom {zoom}; drawing a flat map");
        return Ok(None);
    }
    let mut texture = Pixmap::new(w as u32, h as u32).ok_or("invalid terrain size")?;
    texture.fill(background);
    if let Some(map) = map {
        map.draw(
            &mut texture,
            &Projection::from_origin(zoom, x0, y0),
            opacity,
        );
    }

    let dem_zoom = zoom.saturating_sub(1).min(MAX_DEM_ZOOM);
    let scale = 2f64.powi(dem_zoom as i32 - zoom as i32);
    let last = (1i64 << dem_zoom) - 1;
    let tile = |px: f64| ((px * scale / 256.0).floor() as i64).clamp(0, last) as u32;
    let (tx0, ty0) = (tile(x0 - 1.0 / scale), tile(y0 - 1.0 / scale));
    let (tx1, ty1) = (tile(x0 + w + 1.0 / scale), tile(y0 + h + 1.0 / scale));
    let placements: Vec<TilePlacement> = (ty0..=ty1)
        .flat_map(|y| (tx0..=tx1).map(move |x| (x, y)))
        .map(|(x, y)| TilePlacement {
            id: TileId {
                zoom: dem_zoom,
                x,
                y,
            },
            dx: 0.0,
            dy: 0.0,
            size: 256.0,
        })
        .collect();
    let mut fetcher = TileFetcher::new(template.to_string(), cache)?;
    let mut dem = Dem::new(dem_zoom);
    for (placement, pixmap) in fetcher.fetch_all(&placements) {
        if !dem.add_terrarium(placement.id, pixmap.data()) {
            eprintln!(
                "warning: terrain tile {}/{}/{} is not a 256px Terrarium tile",
                placement.id.zoom, placement.id.x, placement.id.y
            );
        }
    }
    Ok(Some(Terrain::build(&dem, relief, zoom, (x0, y0), texture)))
}

fn world_bounds(points: &[TrackPoint], zoom: u32) -> Option<(f64, f64, f64, f64)> {
    let first = points.first()?;
    let start = (lon_to_pixel(first.lon, zoom), lat_to_pixel(first.lat, zoom));
    Some(points.iter().fold(
        (start.0, start.1, start.0, start.1),
        |(x0, y0, x1, y1), p| {
            let (x, y) = (lon_to_pixel(p.lon, zoom), lat_to_pixel(p.lat, zoom));
            (x0.min(x), y0.min(y), x1.max(x), y1.max(y))
        },
    ))
}
