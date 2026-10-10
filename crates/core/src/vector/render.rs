use tiny_skia::{
    FillRule, LineCap, LineJoin, Paint as SkPaint, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

use super::mvt::{GeomType, VectorTile};
use super::style::{MapStyle, Paint};
use crate::tiles::TilePlacement;

/// Rasterize vector tiles onto `target` with `style`, blended at `opacity`.
///
/// Rules are drawn in style order across every tile, so a road in one tile is
/// never painted under water from its neighbor. Tile buffers overlap at the
/// edges, which is invisible with opaque style colors. Overzoomed placements
/// (`size > 256`) scale the vector geometry, so it stays sharp at any zoom.
pub fn draw_vector_map(
    target: &mut Pixmap,
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    opacity: f32,
) {
    let Some(mut layer) = Pixmap::new(target.width(), target.height()) else {
        return;
    };
    layer.fill(style.background);

    for rule in &style.rules {
        let mut builder = PathBuilder::new();
        let want = match rule.paint {
            Paint::Fill(_) => GeomType::Polygon,
            Paint::Line { .. } => GeomType::LineString,
        };
        for (placement, tile) in tiles {
            let Some(source) = tile.layer(&rule.layer) else {
                continue;
            };
            let scale = placement.size / source.extent.max(1) as f64;
            for feature in source.features.iter().filter(|f| f.geom_type == want) {
                let class = source.property(feature, "class").and_then(|v| v.as_str());
                if !rule.accepts(class) {
                    continue;
                }
                for part in feature.parts() {
                    for (i, &(x, y)) in part.iter().enumerate() {
                        let px = (placement.dx + x as f64 * scale) as f32;
                        let py = (placement.dy + y as f64 * scale) as f32;
                        if i == 0 {
                            builder.move_to(px, py);
                        } else {
                            builder.line_to(px, py);
                        }
                    }
                    if want == GeomType::Polygon {
                        builder.close();
                    }
                }
            }
        }

        let Some(path) = builder.finish() else {
            continue;
        };
        let mut paint = SkPaint {
            anti_alias: true,
            ..Default::default()
        };
        match rule.paint {
            Paint::Fill(color) => {
                paint.set_color(color);
                layer.fill_path(
                    &path,
                    &paint,
                    FillRule::Winding,
                    Transform::identity(),
                    None,
                );
            }
            Paint::Line { color, width } => {
                paint.set_color(color);
                let stroke = Stroke {
                    width,
                    line_cap: LineCap::Round,
                    line_join: LineJoin::Round,
                    ..Default::default()
                };
                layer.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
            }
        }
    }

    target.draw_pixmap(
        0,
        0,
        layer.as_ref(),
        &PixmapPaint {
            opacity: opacity.clamp(0.0, 1.0),
            ..Default::default()
        },
        Transform::identity(),
        None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileId;
    use crate::vector::style::Rule;
    use tiny_skia::Color;

    fn placement(size: f64) -> TilePlacement {
        TilePlacement {
            id: TileId {
                zoom: 0,
                x: 0,
                y: 0,
            },
            dx: 0.0,
            dy: 0.0,
            size,
        }
    }

    fn style(fill: Color) -> MapStyle {
        MapStyle {
            background: Color::BLACK,
            rules: vec![Rule {
                layer: "water".into(),
                classes: Vec::new(),
                paint: Paint::Fill(fill),
            }],
        }
    }

    #[test]
    fn empty_tiles_paint_style_background() {
        let mut target = Pixmap::new(64, 64).unwrap();
        target.fill(Color::WHITE);
        draw_vector_map(&mut target, &[], &style(Color::WHITE), 1.0);
        let px = target.pixel(5, 5).unwrap();
        assert_eq!((px.red(), px.green(), px.blue()), (0, 0, 0));
    }

    #[test]
    fn zero_opacity_keeps_target() {
        let mut target = Pixmap::new(64, 64).unwrap();
        target.fill(Color::WHITE);
        draw_vector_map(&mut target, &[], &style(Color::WHITE), 0.0);
        assert_eq!(target.pixel(5, 5).unwrap().red(), 0xff);
    }

    #[test]
    fn missing_layers_are_skipped() {
        let mut target = Pixmap::new(64, 64).unwrap();
        let tile = VectorTile::default();
        draw_vector_map(
            &mut target,
            &[(placement(64.0), &tile)],
            &style(Color::WHITE),
            1.0,
        );
        assert_eq!(target.pixel(32, 32).unwrap().red(), 0);
    }
}
