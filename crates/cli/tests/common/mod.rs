//! Helpers shared by the CLI integration tests.

// Each test binary uses its own subset of these helpers.
#![allow(dead_code)]

pub mod mvt;
pub mod pmtiles;

use std::path::{Path, PathBuf};

use assert_cmd::Command;
use image::RgbaImage;

/// A public sample track from the repository's `samples/` directory.
pub fn sample(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../samples")
        .join(name)
}

/// The `gpx-motion` binary with its tile cache inside `dir`, so a run never
/// touches `$HOME`.
pub fn gpx_motion(dir: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_gpx-motion"));
    cmd.arg("--tile-cache-dir").arg(dir.join("tiles"));
    cmd
}

/// The decoded PNG at `path`.
pub fn read_png(path: &Path) -> RgbaImage {
    image::open(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
        .to_rgba8()
}

/// How many pixels of `image` are exactly `rgb`.
pub fn count_color(image: &RgbaImage, rgb: [u8; 3]) -> usize {
    image.pixels().filter(|p| p.0[..3] == rgb).count()
}

/// Whether `image` holds more than one color.
pub fn has_several_colors(image: &RgbaImage) -> bool {
    let first = image.get_pixel(0, 0);
    image.pixels().any(|p| p != first)
}

/// The entries of `dir`, by file name.
pub fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}
