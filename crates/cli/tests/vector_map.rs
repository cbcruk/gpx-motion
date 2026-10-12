//! An offline vector map: a PMTiles archive built at test time under the
//! namsan sample, drawn in the `dark` style.

mod common;

use std::f64::consts::PI;
use std::path::Path;

use common::pmtiles::{self, Tile};
use common::{count_color, gpx_motion, mvt, read_png, sample};

/// The `dark` style's water fill.
const DARK_WATER: [u8; 3] = [0x1b, 0x2a, 0x38];

/// Zoom of the fixture's only tile; deeper views overzoom it.
const ZOOM: u8 = 10;

/// The `ZOOM` tile holding `lat`/`lon`.
fn tile_at(lat: f64, lon: f64) -> (u32, u32) {
    let n = f64::from(1u32 << ZOOM);
    let x = (lon + 180.0) / 360.0 * n;
    let lat = lat.to_radians();
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / PI) / 2.0 * n;
    (x as u32, y as u32)
}

/// An archive whose tile under Namsan is all water.
fn write_fixture(path: &Path) {
    let (x, y) = tile_at(37.5509, 126.9882);
    let edge = mvt::EXTENT as i32 + 64;
    let water = mvt::polygon_layer(
        "water",
        &[(-64, -64), (edge, -64), (edge, edge), (-64, edge)],
    );
    let tile = Tile {
        z: ZOOM,
        x,
        y,
        mvt: mvt::tile(&[water]),
    };
    std::fs::write(path, pmtiles::archive(&[tile])).unwrap();
}

fn render(dir: &Path, name: &str, extra: &[&str]) -> image::RgbaImage {
    let output = dir.join(name);
    gpx_motion(dir)
        .arg("-i")
        .arg(sample("namsan.gpx"))
        .arg("-o")
        .arg(&output)
        .args(["--viewport-width", "320", "--viewport-height", "240"])
        .args(extra)
        .assert()
        .success()
        .stderr("");
    read_png(&output)
}

#[test]
fn draws_a_local_pmtiles_archive() {
    let dir = tempfile::tempdir().unwrap();
    let archive = dir.path().join("namsan.pmtiles");
    write_fixture(&archive);
    let archive = archive.to_str().unwrap();

    let plain = render(dir.path(), "plain.png", &[]);
    let mapped = render(
        dir.path(),
        "mapped.png",
        &["--vector-tiles", archive, "--map-style", "dark"],
    );

    let pixels = 320 * 240;
    assert_eq!(count_color(&plain, DARK_WATER), 0);
    let water = count_color(&mapped, DARK_WATER);
    assert!(water > pixels / 2, "{water} of {pixels} pixels are water");
}
