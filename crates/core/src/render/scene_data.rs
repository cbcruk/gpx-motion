//! A 3D scene as plain data for renderers that draw it themselves (e.g. on
//! a GPU in the browser): track vertices raised the same way
//! [`draw_track_3d`](super::draw_track_3d) raises them.

use tiny_skia::Color;

use super::frame::FrameStyle;
use super::ramp::elevation_color;
use super::scene::{densify, Relief, TERRAIN_STEP_PX};
use crate::anim::{Sample, Timeline};
use crate::geo::camera::Camera;
use crate::geo::mercator::{lat_to_pixel, lon_to_pixel};
use crate::terrain::Terrain;

/// A point of the track in world pixels at the camera's zoom.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackVertex {
    /// World x (east).
    pub x: f64,
    /// World y (south).
    pub y: f64,
    /// Height of the raised track in world pixels, from the relief alone.
    pub lift: f64,
    /// Terrain height under the point in world pixels (`0` without terrain);
    /// the track stands on the ground where this exceeds `lift`.
    pub ground: f64,
    /// Progress along the track, `0..=1`.
    pub u: f64,
    /// Color of the segment ending here (the first vertex takes its own).
    pub color: Color,
}

/// The whole track as vertices for `camera`'s world, densified over
/// `terrain` so the line can follow the ground between samples.
///
/// Colors follow the elevation ramp when `style.color_by_elevation` is set
/// and the track has elevation, otherwise `style.tail_color`.
pub fn track_vertices(
    timeline: &Timeline,
    camera: &Camera,
    relief: Relief,
    terrain: Option<&Terrain>,
    style: &FrameStyle,
) -> Vec<TrackVertex> {
    let samples = timeline.window(0.0, 1.0);
    let samples = match terrain {
        Some(_) => densify(samples, camera, TERRAIN_STEP_PX),
        None => samples,
    };
    let elevation = timeline.elevation().filter(|_| style.color_by_elevation);
    let color_of = |a: &Sample, b: &Sample| match (elevation, a.ele, b.ele) {
        (Some(e), Some(ea), Some(eb)) => elevation_color(e.normalize((ea + eb) / 2.0)),
        _ => style.tail_color,
    };
    let mut previous = samples.first().copied();
    samples
        .iter()
        .map(|s| {
            let color = previous.map_or(style.tail_color, |p| color_of(&p, s));
            previous = Some(*s);
            vertex(s, camera, relief, terrain, color)
        })
        .collect()
}

/// Where the marker stands at progress `u`, as [`track_vertices`] places
/// the track (its color is the marker's).
pub fn marker_vertex(
    timeline: &Timeline,
    camera: &Camera,
    relief: Relief,
    terrain: Option<&Terrain>,
    u: f64,
    style: &FrameStyle,
) -> TrackVertex {
    let (lat, lon) = timeline.position_at(u);
    let sample = Sample {
        lat,
        lon,
        ele: timeline.elevation_at(u),
        u,
    };
    vertex(&sample, camera, relief, terrain, style.marker_color)
}

fn vertex(
    s: &Sample,
    camera: &Camera,
    relief: Relief,
    terrain: Option<&Terrain>,
    color: Color,
) -> TrackVertex {
    TrackVertex {
        x: lon_to_pixel(s.lon, camera.zoom),
        y: lat_to_pixel(s.lat, camera.zoom),
        lift: camera.meters_to_px(relief.height_m(s.ele), s.lat),
        ground: terrain.map_or(0.0, |t| t.height_at(s.lat, s.lon)),
        u: s.u,
        color,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Lens;
    use crate::gpx::TrackPoint;

    fn timeline() -> Timeline {
        let mut points = vec![
            TrackPoint::new(37.550, 126.980),
            TrackPoint::new(37.552, 126.984),
            TrackPoint::new(37.556, 126.986),
        ];
        for (p, ele) in points.iter_mut().zip([10.0, 60.0, 30.0]) {
            p.ele = Some(ele);
        }
        Timeline::build(&points).unwrap()
    }

    #[test]
    fn vertices_cover_the_track_in_order_and_match_the_projection() {
        let timeline = timeline();
        let camera = Camera::new(37.553, 126.983, 15.0, 320, 240, Lens::default());
        let relief = Relief::auto(&timeline);
        let style = FrameStyle {
            color_by_elevation: true,
            ..Default::default()
        };
        let vertices = track_vertices(&timeline, &camera, relief, None, &style);
        assert_eq!(vertices.len(), 3);
        assert_eq!(vertices[0].u, 0.0);
        assert_eq!(vertices[2].u, 1.0);
        assert!(vertices.windows(2).all(|v| v[0].u < v[1].u));
        assert!(vertices.iter().all(|v| v.ground == 0.0));
        assert_eq!(vertices[0].lift, 0.0, "the lowest point sits on the map");
        assert!(vertices[1].lift > vertices[2].lift);
        assert_ne!(vertices[1].color, vertices[2].color);
        let (x, y) = camera
            .project_world(vertices[1].x, vertices[1].y, 0.0)
            .unwrap();
        let (px, py) = camera.project(37.552, 126.984, 0.0).unwrap();
        assert!((x - px).abs() < 1e-6 && (y - py).abs() < 1e-6);

        let marker = marker_vertex(&timeline, &camera, relief, None, 0.5, &style);
        assert_eq!(marker.color, style.marker_color);
        assert!(marker.lift > 0.0);
    }
}
