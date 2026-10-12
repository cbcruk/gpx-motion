//! Fixtures shared by the label tests: a style, place points and tiles, and
//! a straight-down camera.

use tiny_skia::Color;

use super::{place_labels, Keepout, LabelMemory, LabelSources, LabelStyle, PlacedLabel};
use crate::geo::Camera;
use crate::render::TextFont;
use crate::tiles::{TileId, TilePlacement};
use crate::vector::mvt::{Layer, TestPoint, Value, VectorTile};

pub(super) fn style() -> LabelStyle {
    LabelStyle {
        font: TextFont::bundled().clone(),
        size: 14.0,
        color: Color::WHITE,
        halo: Color::BLACK,
        language: Some("en".into()),
        pixel_ratio: 1.0,
    }
}

pub(super) fn place(name: &str, class: &str, x: i32, y: i32) -> TestPoint {
    (
        (x, y),
        vec![
            ("name", Value::String(name.to_string())),
            ("class", Value::String(class.to_string())),
        ],
    )
}

pub(super) fn tile(points: &[TestPoint]) -> VectorTile {
    VectorTile {
        layers: vec![Layer::test_points("place", points)],
    }
}

pub(super) fn placement() -> TilePlacement {
    TilePlacement {
        id: TileId {
            zoom: 0,
            x: 0,
            y: 0,
        },
        dx: 0.0,
        dy: 0.0,
        size: 512.0,
    }
}

pub(super) fn run(tile: &VectorTile, keepout: &Keepout) -> Vec<PlacedLabel> {
    let tiles = [(placement(), tile)];
    let project = |p: &TilePlacement, x: f64, y: f64| {
        Some(((p.dx + x * p.size) as f32, (p.dy + y * p.size) as f32, 1.0))
    };
    place_labels(&tiles, &style(), project, (512.0, 512.0), keepout)
}

pub(super) fn straight_down() -> Camera {
    let lens = crate::geo::Lens {
        pitch_deg: 0.0,
        bearing_deg: 0.0,
        fov_deg: 40.0,
    };
    Camera::new(0.0, 0.0, 1.0, 512, 512, lens)
}

pub(super) fn steady(t: &VectorTile, memory: &mut LabelMemory) -> Vec<PlacedLabel> {
    let tiles = [(placement(), t)];
    LabelSources::collect(&tiles, &style()).place_3d_steady(
        &straight_down(),
        &style(),
        (512, 512),
        &Keepout::default(),
        None,
        memory,
    )
}

pub(super) fn names(labels: &[PlacedLabel]) -> Vec<&str> {
    labels.iter().map(|l| l.text.as_str()).collect()
}
