use tiny_skia::{Pixmap, PixmapPaint, Transform};

use super::TilePlacement;

/// Composite decoded tiles onto the target canvas at their viewport positions.
///
/// `opacity` (`0.0..=1.0`) controls background-map visibility; tiles are drawn
/// in the given order, so callers should pass them back-to-front if it matters.
pub fn compose(target: &mut Pixmap, tiles: &[(TilePlacement, Pixmap)], opacity: f32) {
    let paint = PixmapPaint {
        opacity: opacity.clamp(0.0, 1.0),
        ..Default::default()
    };
    for (placement, tile) in tiles {
        target.draw_pixmap(
            placement.dx.round() as i32,
            placement.dy.round() as i32,
            tile.as_ref(),
            &paint,
            Transform::identity(),
            None,
        );
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
}
