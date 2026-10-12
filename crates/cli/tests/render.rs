//! Renders of the public namsan sample without a map: a 2D and a 3D PNG,
//! and an MP4 when ffmpeg is installed.

mod common;

use std::path::Path;
use std::process::Command;

use predicates::prelude::PredicateBooleanExt;
use predicates::str::{contains, is_match};

use common::{entries, gpx_motion, has_several_colors, read_png, sample};

/// The first track's color, from the CLI's palette.
const TRACK_RGB: [u8; 3] = [0xff, 0x5a, 0x36];

/// Whether some pixel is within a few levels of `rgb` on every channel.
fn has_color_near(image: &image::RgbaImage, rgb: [u8; 3]) -> bool {
    image
        .pixels()
        .any(|p| p.0[..3].iter().zip(rgb).all(|(&a, b)| a.abs_diff(b) <= 8))
}

#[test]
fn renders_a_2d_png() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.png");
    gpx_motion(dir.path())
        .arg("-i")
        .arg(sample("namsan.gpx"))
        .arg("-o")
        .arg(&output)
        .args(["--viewport-width", "320", "--viewport-height", "240"])
        .assert()
        .success()
        .stdout(is_match(r"^Rendered 1 track\(s\), \d+ points -> .*out\.png\n$").unwrap());
    let image = read_png(&output);
    assert_eq!(image.dimensions(), (320, 240));
    assert!(has_several_colors(&image));
    assert!(has_color_near(&image, TRACK_RGB), "no track pixels");
    assert_eq!(entries(dir.path()), ["out.png"]);
}

#[test]
fn renders_a_3d_png() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.png");
    gpx_motion(dir.path())
        .arg("-i")
        .arg(sample("namsan.gpx"))
        .arg("-o")
        .arg(&output)
        .args(["--viewport-width", "320", "--viewport-height", "240"])
        .args(["--pitch", "55", "--bearing", "20"])
        .assert()
        .success()
        .stdout(contains("in 3D"));
    let image = read_png(&output);
    assert_eq!(image.dimensions(), (320, 240));
    assert!(has_several_colors(&image));
    assert!(has_color_near(&image, TRACK_RGB), "no track pixels");
}

/// Whether `tool -version` runs.
fn installed(tool: &str) -> bool {
    Command::new(tool)
        .arg("-version")
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `key=value` lines from ffprobe for the video stream's decoded frame
/// count and the container duration.
fn probe(path: &Path) -> (u64, f64) {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-count_frames", "-select_streams", "v:0"])
        .args(["-show_entries", "stream=nb_read_frames:format=duration"])
        .args(["-of", "default=noprint_wrappers=1"])
        .arg(path)
        .output()
        .unwrap();
    assert!(out.status.success(), "ffprobe failed");
    let text = String::from_utf8(out.stdout).unwrap();
    let field = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .unwrap_or_else(|| panic!("no {key} in {text}"))
            .to_string()
    };
    (
        field("nb_read_frames").parse().unwrap(),
        field("duration").parse().unwrap(),
    )
}

#[test]
fn encodes_an_mp4_with_ffmpeg() {
    if !installed("ffmpeg") {
        eprintln!("note: ffmpeg not found on PATH; skipping the MP4 render");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.mp4");
    gpx_motion(dir.path())
        .arg("-i")
        .arg(sample("namsan.gpx"))
        .arg("-o")
        .arg(&output)
        .args(["--viewport-width", "320", "--viewport-height", "240"])
        .args(["--total-time", "1000", "--fps", "5"])
        .assert()
        .success()
        .stdout(contains("Encoded 5 frames @ 5fps, 1 track(s)").and(contains("(1.0s)")));
    assert!(std::fs::metadata(&output).unwrap().len() > 0);
    assert_eq!(entries(dir.path()), ["out.mp4"], "a partial file remains");
    if !installed("ffprobe") {
        eprintln!("note: ffprobe not found on PATH; skipping the frame count check");
        return;
    }
    let (frames, duration) = probe(&output);
    assert_eq!(frames, 5);
    assert!((duration - 1.0).abs() < 0.05, "duration {duration}");
}
