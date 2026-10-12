//! Benchmarks for the core's hot paths, through its public API only.
//!
//! Inputs are the public `samples/namsan.gpx` track, a synthetic hill under
//! it, and synthetic city tiles, so a run needs no network. Run with
//! `cargo bench -p gpx-motion-core`; pass a filter to run a subset, e.g.
//! `cargo bench -p gpx-motion-core -- labels`.

mod fixtures;
mod mvt;

fn main() {
    divan::main();
}

/// Reading GPX and preparing a track for animation.
mod track {
    use divan::{black_box, Bencher};
    use gpx_motion_core::{parse, Timeline};

    use crate::fixtures::{namsan, namsan_timeline, NAMSAN_GPX};

    /// Frames sampled from the timeline per iteration, as in a 10 s video
    /// at 60 fps.
    const FRAMES: u32 = 600;

    #[divan::bench]
    fn parse_namsan() -> usize {
        parse(black_box(NAMSAN_GPX)).map_or(0, |p| p.len())
    }

    #[divan::bench]
    fn build_timeline(bencher: Bencher) {
        let points = namsan();
        bencher.bench(|| Timeline::build(black_box(&points)));
    }

    /// Position, heading and elevation at every frame of a video.
    #[divan::bench]
    fn sample_frames(bencher: Bencher) {
        let timeline = namsan_timeline();
        bencher.bench(|| {
            let mut sum = 0.0;
            for f in 0..FRAMES {
                let u = f64::from(f) / f64::from(FRAMES - 1);
                let (x, y) = timeline.position_at(u);
                sum += x + y;
                sum += timeline.heading_at(u, 50.0).unwrap_or(0.0);
                sum += timeline.elevation_at(u).unwrap_or(0.0);
            }
            sum
        });
    }
}

/// One 3D frame's terrain and track, which the triangle rasterizer and the
/// track's curtains dominate.
mod scene {
    use divan::Bencher;
    use gpx_motion_core::{draw_terrain, draw_track_3d, FrameStyle, SceneView};
    use tiny_skia::Pixmap;

    use crate::fixtures::{namsan_camera, namsan_terrain, namsan_timeline, FRAME, RELIEF};

    fn blank() -> Pixmap {
        Pixmap::new(FRAME.0, FRAME.1).expect("frame size is not zero")
    }

    /// The hill's textured, hill-shaded mesh, rasterized with a depth map.
    #[divan::bench]
    fn terrain(bencher: Bencher) {
        let camera = namsan_camera();
        let terrain = namsan_terrain(&camera);
        bencher
            .with_inputs(blank)
            .bench_local_refs(|pixmap| draw_terrain(pixmap, &camera, &terrain));
    }

    /// The track at mid-progress with a 30 % tail, on a flat map.
    #[divan::bench]
    fn track_flat(bencher: Bencher) {
        let camera = namsan_camera();
        let timeline = namsan_timeline();
        let style = FrameStyle::default();
        let view = SceneView {
            camera: &camera,
            relief: RELIEF,
            terrain: None,
            occlusion: None,
        };
        bencher.with_inputs(blank).bench_local_refs(|pixmap| {
            draw_track_3d(pixmap, &timeline, &view, 0.5, 0.3, &style);
        });
    }

    /// The same track standing on the hill and hidden behind it per pixel.
    #[divan::bench]
    fn track_over_terrain(bencher: Bencher) {
        let camera = namsan_camera();
        let terrain = namsan_terrain(&camera);
        let timeline = namsan_timeline();
        let style = FrameStyle::default();
        let mut base = blank();
        let depth = draw_terrain(&mut base, &camera, &terrain);
        let view = SceneView {
            camera: &camera,
            relief: RELIEF,
            terrain: Some(&terrain),
            occlusion: Some(&depth),
        };
        bencher
            .with_inputs(|| base.clone())
            .bench_local_refs(|pixmap| {
                draw_track_3d(pixmap, &timeline, &view, 0.5, 0.3, &style);
            });
    }
}

/// Painting vector tiles with the dark preset.
mod vector_map {
    use divan::Bencher;
    use gpx_motion_core::{draw_vector_map, draw_vector_map_3d, MapStyle, TileId, TilePlacement};
    use tiny_skia::Pixmap;

    use crate::fixtures::{borrowed, city_tiles, namsan_camera, FRAME};

    /// Four overzoomed tiles filling a 1024 px square, as a 2D frame draws
    /// them.
    #[divan::bench]
    fn paint_flat(bencher: Bencher) {
        let placements: Vec<TilePlacement> = (0..4)
            .map(|i| TilePlacement {
                id: TileId {
                    zoom: 14,
                    x: 13_970 + i % 2,
                    y: 6_344 + i / 2,
                },
                dx: f64::from(i % 2) * 512.0,
                dy: f64::from(i / 2) * 512.0,
                size: 512.0,
            })
            .collect();
        let tiles = city_tiles(&placements);
        let tiles = borrowed(&tiles);
        let style = MapStyle::dark();
        bencher
            .with_inputs(|| Pixmap::new(1024, 1024).expect("size is not zero"))
            .bench_local_refs(|pixmap| draw_vector_map(pixmap, &tiles, None, &style, 1.0));
    }

    /// The ground tiles of the pitched namsan camera, in perspective.
    #[divan::bench]
    fn paint_3d(bencher: Bencher) {
        let camera = namsan_camera();
        let tiles = city_tiles(&camera.ground_tiles(16));
        let tiles = borrowed(&tiles);
        let style = MapStyle::dark();
        bencher
            .with_inputs(|| Pixmap::new(FRAME.0, FRAME.1).expect("size is not zero"))
            .bench_local_refs(|pixmap| {
                draw_vector_map_3d(pixmap, &tiles, None, &style, &camera, 1.0);
            });
    }
}

/// Map labels for a 3D frame: reading them from tiles, then placing them.
mod labels {
    use divan::Bencher;
    use gpx_motion_core::{Keepout, LabelMemory, LabelSources};

    use crate::fixtures::{
        borrowed, city_tiles, label_style, namsan_camera, namsan_terrain, FRAME,
    };

    /// Point and line labels from the camera's ground tiles.
    #[divan::bench]
    fn collect(bencher: Bencher) {
        let camera = namsan_camera();
        let tiles = city_tiles(&camera.ground_tiles(16));
        let tiles = borrowed(&tiles);
        let style = label_style();
        bencher.bench(|| LabelSources::collect(&tiles, &style));
    }

    /// One frame's placement over the hill, steadied by a memory kept
    /// across frames as a video render keeps it.
    #[divan::bench]
    fn place_3d_steady(bencher: Bencher) {
        let camera = namsan_camera();
        let terrain = namsan_terrain(&camera);
        let tiles = city_tiles(&camera.ground_tiles(16));
        let style = label_style();
        let sources = LabelSources::collect(&borrowed(&tiles), &style);
        let keepout = Keepout::default();
        let mut memory = LabelMemory::new(8);
        let placed = sources.place_3d_steady(
            &camera,
            &style,
            FRAME,
            &keepout,
            Some(&terrain),
            &mut memory,
        );
        assert!(!placed.is_empty(), "the view shows some labels");
        bencher.bench_local(|| {
            sources.place_3d_steady(
                &camera,
                &style,
                FRAME,
                &keepout,
                Some(&terrain),
                &mut memory,
            )
        });
    }
}
