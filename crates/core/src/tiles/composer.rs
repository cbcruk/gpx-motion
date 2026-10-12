//! Compositing decoded raster tiles onto a canvas at their viewport
//! positions.

use tiny_skia::{FilterQuality, Pixmap, PixmapPaint, Transform};

use super::TilePlacement;

/// Composite decoded tiles onto the target canvas at their viewport positions.
///
/// Tiles are scaled from their pixel size to [`TilePlacement::size`] (e.g. at
/// a fractional zoom, or overzoomed past the source's maximum), bilinearly;
/// exact 1:1 placements are drawn unfiltered on whole pixels. `opacity`
/// (`0.0..=1.0`) controls background-map visibility; tiles are drawn in the
/// given order, so callers should pass them back-to-front if it matters.
pub fn compose(target: &mut Pixmap, tiles: &[(TilePlacement, Pixmap)], opacity: f32) {
    let opacity = opacity.clamp(0.0, 1.0);
    for (placement, tile) in tiles {
        let scale = placement.size / f64::from(tile.width().max(1));
        if (scale - 1.0).abs() < 1e-9 {
            let paint = PixmapPaint {
                opacity,
                ..Default::default()
            };
            target.draw_pixmap(
                placement.dx.round() as i32,
                placement.dy.round() as i32,
                tile.as_ref(),
                &paint,
                Transform::identity(),
                None,
            );
            continue;
        }
        let paint = PixmapPaint {
            opacity,
            quality: FilterQuality::Bilinear,
            ..Default::default()
        };
        let transform = Transform::from_row(
            scale as f32,
            0.0,
            0.0,
            scale as f32,
            placement.dx as f32,
            placement.dy as f32,
        );
        target.draw_pixmap(0, 0, tile.as_ref(), &paint, transform, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileId;
    use tiny_skia::Color;

    fn solid_tile(color: Color) -> Pixmap {
        let mut p = Pixmap::new(256, 256).unwrap();
        p.fill(color);
        p
    }

    fn placement(dx: f64, dy: f64) -> TilePlacement {
        TilePlacement {
            id: TileId {
                zoom: 0,
                x: 0,
                y: 0,
            },
            dx,
            dy,
            size: 256.0,
        }
    }

    #[test]
    fn opaque_tile_overwrites_target() {
        let mut target = Pixmap::new(256, 256).unwrap();
        target.fill(Color::BLACK);
        let red = Color::from_rgba8(0xff, 0, 0, 0xff);
        compose(&mut target, &[(placement(0.0, 0.0), solid_tile(red))], 1.0);
        let px = target.pixel(10, 10).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0xff, 0, 0));
    }

    #[test]
    fn zero_opacity_leaves_target_untouched() {
        let mut target = Pixmap::new(256, 256).unwrap();
        target.fill(Color::BLACK);
        let red = Color::from_rgba8(0xff, 0, 0, 0xff);
        compose(&mut target, &[(placement(0.0, 0.0), solid_tile(red))], 0.0);
        let px = target.pixel(10, 10).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0, 0, 0));
    }

    #[test]
    fn offset_placement_only_covers_its_region() {
        let mut target = Pixmap::new(512, 512).unwrap();
        target.fill(Color::BLACK);
        let red = Color::from_rgba8(0xff, 0, 0, 0xff);
        compose(
            &mut target,
            &[(placement(256.0, 256.0), solid_tile(red))],
            1.0,
        );
        assert_eq!(target.pixel(10, 10).unwrap().red(), 0);
        assert_eq!(target.pixel(300, 300).unwrap().red(), 0xff);
    }

    #[test]
    fn scaled_placements_cover_their_size() {
        let mut target = Pixmap::new(400, 400).unwrap();
        target.fill(Color::BLACK);
        let red = Color::from_rgba8(0xff, 0, 0, 0xff);
        let mut p = placement(10.0, 10.0);
        p.size = 362.0;
        compose(&mut target, &[(p, solid_tile(red))], 1.0);
        assert_eq!(target.pixel(360, 360).unwrap().red(), 0xff);
        assert_eq!(target.pixel(380, 380).unwrap().red(), 0);
    }
}
