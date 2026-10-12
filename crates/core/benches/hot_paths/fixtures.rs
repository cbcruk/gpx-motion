//! Inputs for the benchmarks: the public namsan sample, a synthetic hill
//! under it, and synthetic city tiles. Nothing here touches the network.

use gpx_motion_core::mercator::{lat_to_pixel, lon_to_pixel};
use gpx_motion_core::{
    parse, Camera, Dem, LabelStyle, Lens, MapStyle, Relief, Terrain, TextFont, TileId,
    TilePlacement, Timeline, TrackPoint, VectorTile,
};
use tiny_skia::{Color, Pixmap};

use crate::mvt::{tile, Geometry, Layer, EXTENT};

/// The namsan sample track (153 points with elevations).
pub const NAMSAN_GPX: &str = include_str!("../../../../samples/namsan.gpx");

/// Width and height of the 3D frames.
pub const FRAME: (u32, u32) = (960, 540);

/// Zoom of the DEM tiles.
const DEM_ZOOM: u32 = 13;

/// Peak of the synthetic hill: N Seoul Tower on Namsan.
const PEAK: (f64, f64) = (37.5512, 126.9882);

/// The namsan track's points.
pub fn namsan() -> Vec<TrackPoint> {
    parse(NAMSAN_GPX).expect("namsan.gpx parses")
}

/// The namsan track's timeline.
pub fn namsan_timeline() -> Timeline {
    Timeline::build(&namsan()).expect("namsan has distinct points")
}

/// Relief the track and terrain are raised with.
pub const RELIEF: Relief = Relief {
    base_m: 0.0,
    exaggeration: 1.5,
};

/// A pitched, rotated camera fitted to the namsan track.
pub fn namsan_camera() -> Camera {
    let lens = Lens {
        pitch_deg: 60.0,
        bearing_deg: 30.0,
        ..Lens::default()
    };
    Camera::fit(&namsan(), &[], FRAME.0, FRAME.1, 32.0, 0.0, lens).expect("track is not empty")
}

/// Elevation of the synthetic hill at a zoom-13 world pixel: a 260 m dome
/// with ridges, so the mesh has slopes in every direction.
fn hill_height(x: f64, y: f64) -> f64 {
    let z = f64::from(DEM_ZOOM);
    let (px, py) = (lon_to_pixel(PEAK.1, z), lat_to_pixel(PEAK.0, z));
    let d2 = ((x - px).powi(2) + (y - py).powi(2)) / 90.0_f64.powi(2);
    let ridges = 25.0 * (x / 9.0).sin() * (y / 7.0).cos();
    (260.0 * (-d2).exp() + ridges + 30.0).max(0.0)
}

/// A Terrarium tile of the synthetic hill.
fn hill_tile(tx: u32, ty: u32) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(256 * 256 * 4);
    for py in 0..256 {
        for px in 0..256 {
            let x = f64::from(tx * 256 + px);
            let y = f64::from(ty * 256 + py);
            let v = hill_height(x, y) + 32768.0;
            let r = (v / 256.0).floor();
            let g = (v - r * 256.0).floor();
            rgba.extend_from_slice(&[r as u8, g as u8, 0, 0xff]);
        }
    }
    rgba
}

/// Terrain under the namsan camera: the synthetic hill over the track's
/// bounds plus a margin, at the camera's zoom.
pub fn namsan_terrain(camera: &Camera) -> Terrain {
    let points = namsan();
    let z = f64::from(DEM_ZOOM);
    let xs = points.iter().map(|p| lon_to_pixel(p.lon, z));
    let ys = points.iter().map(|p| lat_to_pixel(p.lat, z));
    let (x0, x1) = xs.fold((f64::MAX, f64::MIN), |(a, b), x| (a.min(x), b.max(x)));
    let (y0, y1) = ys.fold((f64::MAX, f64::MIN), |(a, b), y| (a.min(y), b.max(y)));
    let margin = (x1 - x0).max(y1 - y0) * 0.5;
    let (x0, y0, x1, y1) = (x0 - margin, y0 - margin, x1 + margin, y1 + margin);

    let mut dem = Dem::new(DEM_ZOOM);
    for ty in (y0 / 256.0) as u32..=(y1 / 256.0) as u32 {
        for tx in (x0 / 256.0) as u32..=(x1 / 256.0) as u32 {
            let id = TileId {
                zoom: DEM_ZOOM,
                x: tx,
                y: ty,
            };
            dem.add_terrarium(id, &hill_tile(tx, ty));
        }
    }
    let scale = 2f64.powf(camera.zoom - z);
    let origin = (x0 * scale, y0 * scale);
    let size = ((x1 - x0) * scale, (y1 - y0) * scale);
    let mut map = Pixmap::new(size.0 as u32, size.1 as u32).expect("map size is not zero");
    map.fill(Color::from_rgba8(0x22, 0x28, 0x22, 0xff));
    Terrain::build(&dem, &RELIEF, camera.zoom, origin, map)
}

/// A small deterministic generator, so every run draws the same tiles.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    /// A value in `lo..hi`.
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo) as u32) as i32
    }
}

/// A closed ring of `n` points around `(cx, cy)` with radii in `r`.
fn blob(rng: &mut Lcg, cx: i32, cy: i32, r: (i32, i32), n: usize) -> Vec<(i32, i32)> {
    (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            let rr = f64::from(rng.range(r.0, r.1));
            (cx + (rr * a.cos()) as i32, cy + (rr * a.sin()) as i32)
        })
        .collect()
}

/// A wiggly line across the tile, horizontal or vertical at `at`.
fn street(rng: &mut Lcg, at: i32, horizontal: bool) -> Vec<(i32, i32)> {
    (0..=16)
        .map(|i| {
            let along = -64 + i * (EXTENT + 128) / 16;
            let off = at + rng.range(-40, 40);
            if horizontal {
                (along, off)
            } else {
                (off, along)
            }
        })
        .collect()
}

/// A dense synthetic city tile in the OpenMapTiles schema: land cover,
/// parks, water, hundreds of buildings, a street grid of every road class,
/// and named places and roads (Latin names, unique to the tile).
pub fn city_tile(id: TileId) -> Vec<u8> {
    let mut rng = Lcg(u64::from(id.x) << 32 | u64::from(id.y));
    let mut landcover = Layer::new("landcover");
    let mut park = Layer::new("park");
    let mut water = Layer::new("water");
    let mut waterway = Layer::new("waterway");
    let mut building = Layer::new("building");
    let mut roads = Layer::new("transportation");
    let mut road_names = Layer::new("transportation_name");
    let mut places = Layer::new("place");
    let mut peaks = Layer::new("mountain_peak");

    for _ in 0..6 {
        let (cx, cy) = (rng.range(0, EXTENT), rng.range(0, EXTENT));
        landcover.add(
            Geometry::Polygon(blob(&mut rng, cx, cy, (300, 900), 24)),
            &[],
        );
    }
    for _ in 0..4 {
        let (cx, cy) = (rng.range(0, EXTENT), rng.range(0, EXTENT));
        park.add(
            Geometry::Polygon(blob(&mut rng, cx, cy, (200, 500), 16)),
            &[],
        );
    }
    water.add(
        Geometry::Polygon(blob(&mut rng, 2048, 2048, (250, 400), 32)),
        &[],
    );
    waterway.add(Geometry::Line(street(&mut rng, 3000, true)), &[]);

    for gy in 0..24 {
        for gx in 0..24 {
            let (x, y) = (gx * 170 + rng.range(10, 40), gy * 170 + rng.range(10, 40));
            let (w, h) = (rng.range(40, 120), rng.range(40, 120));
            let ring = vec![(x, y), (x + w, y), (x + w, y + h), (x, y + h)];
            building.add(Geometry::Polygon(ring), &[]);
        }
    }

    let classes = [
        "minor",
        "service",
        "tertiary",
        "secondary",
        "primary",
        "trunk",
        "motorway",
        "path",
    ];
    for i in 0..16 {
        let class = classes[i as usize % classes.len()];
        let at = i * EXTENT / 16 + 128;
        for horizontal in [true, false] {
            let line = street(&mut rng, at, horizontal);
            roads.add(Geometry::Line(line.clone()), &[("class", class)]);
            if i % 3 == 0 && class != "path" {
                let name = format!(
                    "{} Road {}-{}",
                    if horizontal { "East" } else { "North" },
                    id.x % 100,
                    i
                );
                road_names.add(Geometry::Line(line), &[("class", class), ("name", &name)]);
            }
        }
    }

    let place_classes = [
        "city",
        "town",
        "suburb",
        "village",
        "neighbourhood",
        "hamlet",
    ];
    for i in 0..24 {
        let class = place_classes[i % place_classes.len()];
        let name = format!("Place {}-{}-{i}", id.x % 1000, id.y % 1000);
        let (x, y) = (rng.range(64, EXTENT - 64), rng.range(64, EXTENT - 64));
        places.add(Geometry::Point(x, y), &[("class", class), ("name", &name)]);
    }
    for i in 0..4 {
        let name = format!("Peak {}-{}-{i}", id.x % 1000, id.y % 1000);
        let (x, y) = (rng.range(64, EXTENT - 64), rng.range(64, EXTENT - 64));
        peaks.add(Geometry::Point(x, y), &[("name", &name)]);
    }

    tile(&[
        landcover, park, water, waterway, building, roads, road_names, places, peaks,
    ])
}

/// Decoded city tiles for each placement.
pub fn city_tiles(placements: &[TilePlacement]) -> Vec<(TilePlacement, VectorTile)> {
    placements
        .iter()
        .map(|p| {
            let tile = VectorTile::decode(&city_tile(p.id)).expect("city tile decodes");
            (*p, tile)
        })
        .collect()
}

/// Borrowed `(placement, tile)` pairs, as the renderers take them.
pub fn borrowed(tiles: &[(TilePlacement, VectorTile)]) -> Vec<(TilePlacement, &VectorTile)> {
    tiles.iter().map(|(p, t)| (*p, t)).collect()
}

/// Label style with the crate's bundled Latin font over the dark preset.
pub fn label_style() -> LabelStyle {
    let font = TextFont::from_bytes(include_bytes!("../../assets/font.ttf").to_vec(), 0)
        .expect("bundled font loads");
    LabelStyle::contrasting(
        font,
        LabelStyle::CLI_DEFAULT_SIZE,
        MapStyle::dark().background,
        None,
        1.0,
    )
}
