//! A 400 m hill: its DEM tile, the terrain built from it, and a camera
//! looking at it.

use tiny_skia::{Color, Pixmap};

use crate::geo::camera::Lens;
use crate::geo::mercator::{pixel_to_lat, pixel_to_lon, TILE_SIZE};
use crate::geo::Camera;
use crate::render::Relief;
use crate::terrain::{Dem, Terrain};
use crate::tiles::TileId;

/// Zoom of the hill terrain and camera.
pub(crate) const ZOOM: f64 = 14.0;

/// One DEM tile at zoom 13 holding a 400 m hill centered on the tile.
pub(crate) fn hill_dem() -> (Dem, (f64, f64)) {
    let (tx, ty) = (6985, 3172);
    let mut rgba = Vec::with_capacity(256 * 256 * 4);
    for y in 0..256 {
        for x in 0..256 {
            let d = ((x as f64 - 128.0).powi(2) + (y as f64 - 128.0).powi(2)).sqrt();
            let v = (400.0 * (1.0 - d / 128.0)).max(0.0) + 32768.0;
            let r = (v / 256.0).floor();
            let g = (v - r * 256.0).floor();
            rgba.extend_from_slice(&[r as u8, g as u8, 0, 0xff]);
        }
    }
    let mut dem = Dem::new(13);
    dem.add_terrarium(
        TileId {
            zoom: 13,
            x: tx,
            y: ty,
        },
        &rgba,
    );
    let origin = (tx as f64 * 512.0, ty as f64 * 512.0);
    (dem, origin)
}

/// A 512 px square of terrain at zoom 14 with a 400 m hill in the
/// middle, and its world origin.
pub(crate) fn terrain() -> Terrain {
    let (dem, origin) = hill_dem();
    let relief = Relief {
        base_m: 0.0,
        exaggeration: 1.0,
    };
    let mut map = Pixmap::new(512, 512).unwrap();
    map.fill(Color::from_rgba8(0x40, 0x80, 0x40, 0xff));
    Terrain::build(&dem, &relief, ZOOM, origin, map)
}

/// A camera facing north at the hill's summit from `pitch` degrees.
pub(crate) fn hill_camera(pitch_deg: f64) -> Camera {
    let lon = pixel_to_lon(6985.5 * TILE_SIZE, 13.0);
    let lat = pixel_to_lat(3172.5 * TILE_SIZE, 13.0);
    let mut cam = Camera::new(
        lat,
        lon,
        ZOOM,
        320,
        240,
        Lens {
            pitch_deg,
            ..Lens::default()
        },
    );
    cam.distance *= 1.5;
    cam
}

/// World origin of [`terrain`].
pub(crate) fn hill_origin() -> (f64, f64) {
    hill_dem().1
}
