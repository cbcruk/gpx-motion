//! Option errors: each run exits non-zero with a message naming the problem
//! and writes nothing. None of these need the network or ffmpeg.

mod common;

use predicates::str::contains;

use common::{entries, gpx_motion, sample};

/// Exit code for values clap rejects while parsing.
const USAGE: i32 = 2;

/// Exit code for options rejected together, after parsing.
const INVALID: i32 = 1;

/// Run with `extra` options writing to `output` (a name inside a fresh
/// directory, or `""` for the directory itself), and expect `code`, a
/// `message` on stderr, and an empty directory afterwards.
fn rejects(output: &str, extra: &[&str], code: i32, message: &str) {
    let dir = tempfile::tempdir().unwrap();
    gpx_motion(dir.path())
        .arg("-o")
        .arg(dir.path().join(output))
        .arg("-i")
        .arg(sample("namsan.gpx"))
        .args(extra)
        .assert()
        .code(code)
        .stdout("")
        .stderr(contains(message));
    assert_eq!(entries(dir.path()), Vec::<String>::new(), "{extra:?}");
}

#[test]
fn zoom_out_of_range() {
    rejects(
        "out.png",
        &["--zoom", "23"],
        USAGE,
        "must be between 0 and 22",
    );
}

#[test]
fn pitch_out_of_range() {
    rejects(
        "out.png",
        &["--pitch", "76"],
        USAGE,
        "must be between 0 and 75",
    );
}

#[test]
fn fov_out_of_range() {
    rejects(
        "out.png",
        &["--fov", "5"],
        USAGE,
        "must be between 10 and 120",
    );
}

#[test]
fn fps_not_positive() {
    rejects("out.mp4", &["--fps", "0"], USAGE, "must be positive");
}

#[test]
fn viewport_out_of_range() {
    rejects(
        "out.png",
        &["--viewport-width", "0"],
        USAGE,
        "--viewport-width",
    );
    rejects(
        "out.png",
        &["--viewport-height", "16385"],
        USAGE,
        "16385 is not in 1..=16384",
    );
}

#[test]
fn pixel_ratio_out_of_range() {
    rejects(
        "out.png",
        &["--pixel-ratio", "9"],
        USAGE,
        "must be between 0.25 and 8",
    );
}

#[test]
fn label_size_out_of_range() {
    rejects(
        "out.png",
        &["--label-size", "3"],
        USAGE,
        "must be between 4 and 96",
    );
}

#[test]
fn background_map_visibility_out_of_range() {
    rejects(
        "out.png",
        &["--background-map-visibility", "1.5"],
        USAGE,
        "must be between 0 and 1",
    );
}

#[test]
fn unknown_overlay_position() {
    rejects(
        "out.png",
        &["--information-position", "middle"],
        USAGE,
        "possible values",
    );
}

#[test]
fn label_font_without_labels() {
    rejects("out.png", &["--label-font", "font.ttf"], USAGE, "--labels");
}

#[test]
fn video_only_options_on_png() {
    for (extra, flag) in [
        (&["--elevation-profile"][..], "--elevation-profile"),
        (&["--pre-draw-track"][..], "--pre-draw-track"),
        (&["--tail-duration", "1000"][..], "--tail-duration"),
    ] {
        rejects(
            "out.png",
            extra,
            INVALID,
            &format!("{flag} only applies to video output"),
        );
    }
}

#[test]
fn zoom_transition_needs_zoom() {
    rejects(
        "out.mp4",
        &["--zoom-transition", "2"],
        INVALID,
        "--zoom-transition needs --zoom",
    );
}

#[test]
fn camera_options_need_pitch() {
    rejects(
        "out.png",
        &["--follow-heading"],
        INVALID,
        "--follow-heading needs the 3D view (--pitch > 0)",
    );
    rejects(
        "out.png",
        &["--terrain-tiles", "https://t.invalid/{z}/{x}/{y}.png"],
        INVALID,
        "--terrain-tiles needs the 3D view (--pitch > 0)",
    );
}

#[test]
fn the_3d_view_rejects_raster_tiles() {
    rejects(
        "out.png",
        &[
            "--pitch",
            "30",
            "--tms-url-template",
            "https://t.invalid/{zoom}/{x}/{y}.png",
        ],
        INVALID,
        "the 3D view needs --vector-tiles",
    );
}

#[test]
fn labels_need_a_vector_map() {
    rejects(
        "out.png",
        &["--labels"],
        INVALID,
        "--labels needs a vector map (--vector-tiles)",
    );
}

#[test]
fn output_is_a_directory() {
    rejects("", &[], INVALID, "is a directory");
}

#[test]
fn odd_viewport_for_mp4() {
    rejects(
        "out.mp4",
        &["--viewport-width", "321"],
        INVALID,
        "--viewport-width must be even for MP4 output",
    );
}

#[test]
fn missing_input_file() {
    rejects(
        "out.png",
        &["-i", "no-such-track.gpx"],
        INVALID,
        "no-such-track.gpx",
    );
}

#[test]
fn missing_style_file() {
    rejects(
        "out.png",
        &[
            "--vector-tiles",
            "https://t.invalid/{z}/{x}/{y}.pbf",
            "--map-style",
            "./no-such-style.json",
        ],
        INVALID,
        "no-such-style.json: style file not found",
    );
}
