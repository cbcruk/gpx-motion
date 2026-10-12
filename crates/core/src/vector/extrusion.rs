//! Buildings raised into 3D by a style's extrusion rules (MapLibre's
//! `fill-extrusion`): footprints from vector tiles, cut into flat-shaded
//! triangles for a 3D renderer.

use earcut::Earcut;
use serde_json::Value as Json;
use tiny_skia::Color;

use super::mvt::{Feature, GeomType, Layer, VectorTile};
use super::render::clip_to_rect;
use super::style::{MapStyle, Paint};
use crate::geo::mercator::{self, pixel_to_lat};
use crate::tiles::TilePlacement;

/// A height of extruded features in meters: a constant, or a numeric
/// feature property (features without it stand at `0`).
#[derive(Debug, Clone, PartialEq)]
pub enum Height {
    Meters(f64),
    Property(String),
}

impl Height {
    /// Parse a number of meters or a property name.
    pub fn parse(json: &Json) -> Result<Self, String> {
        match json {
            Json::String(key) => Ok(Height::Property(key.clone())),
            _ => json
                .as_f64()
                .filter(|m| m.is_finite() && *m >= 0.0)
                .map(Height::Meters)
                .ok_or_else(|| format!("unsupported height `{json}` (need meters or a property)")),
        }
    }

    /// The height as gpx-motion style JSON.
    pub fn to_json(&self) -> Json {
        match self {
            Height::Meters(m) => (*m).into(),
            Height::Property(key) => key.clone().into(),
        }
    }

    /// The height of `feature` of `layer`, in meters.
    fn of(&self, layer: &Layer, feature: &Feature) -> f64 {
        let meters = match self {
            Height::Meters(m) => *m,
            Height::Property(key) => layer
                .property(feature, key)
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0),
        };
        if meters.is_finite() {
            meters.clamp(0.0, MAX_HEIGHT_M)
        } else {
            0.0
        }
    }
}

/// Tallest extrusion, in meters (above the tallest building).
const MAX_HEIGHT_M: f64 = 1000.0;

/// How a fill rule raises its polygons in 3D views.
#[derive(Debug, Clone, PartialEq)]
pub struct Extrusion {
    /// Height of the roof above the ground.
    pub height: Height,
    /// Height of the walls' foot above the ground.
    pub base: Height,
    /// Darken low walls (MapLibre's `fill-extrusion-vertical-gradient`).
    pub gradient: bool,
}

impl Extrusion {
    /// Parse gpx-motion style JSON: `{"height", "base", "gradient"}`, each
    /// optional (`0`, `0`, `true`).
    pub fn parse(json: &Json) -> Result<Self, String> {
        let object = json
            .as_object()
            .ok_or("an extrusion must be an object with `height`, `base`, `gradient`")?;
        if let Some(key) = object
            .keys()
            .find(|k| !["height", "base", "gradient"].contains(&k.as_str()))
        {
            return Err(format!(
                "unknown key `{key}` (expected height, base, gradient)"
            ));
        }
        let height = |key: &str| {
            object
                .get(key)
                .map_or(Ok(Height::Meters(0.0)), Height::parse)
                .map_err(|e| format!("`{key}`: {e}"))
        };
        let gradient = match object.get("gradient") {
            None => true,
            Some(g) => g.as_bool().ok_or("`gradient` must be true or false")?,
        };
        Ok(Self {
            height: height("height")?,
            base: height("base")?,
            gradient,
        })
    }

    /// The extrusion as gpx-motion style JSON.
    pub fn to_json(&self) -> Json {
        let mut out = serde_json::Map::new();
        out.insert("height".into(), self.height.to_json());
        out.insert("base".into(), self.base.to_json());
        if !self.gradient {
            out.insert("gradient".into(), false.into());
        }
        out.into()
    }
}

/// One raised polygon in world pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Prism {
    /// The outline (clockwise with y down) then its holes (the other way),
    /// each without a closing point. A building cut by a tile edge keeps a
    /// wall along the cut: hidden inside it when the other piece stands as
    /// high, closing the step between them when terrain lifts one higher.
    pub rings: Vec<Vec<(f64, f64)>>,
    /// Edges (from point `i` of a ring to the next) along a cut, which get
    /// walls only on [`Prism::cut_walls`]: clipping a concave building
    /// leaves zero-width bridges there between its pieces. Missing entries
    /// are ordinary walls.
    on_cut: Vec<Vec<bool>>,
    /// The parts of the cut edges with the building on their inner side,
    /// each `(from, to)` along its edge.
    cut_walls: Vec<((f64, f64), (f64, f64))>,
    /// Foot of the walls above the ground, in meters.
    pub base_m: f64,
    /// Roof above the ground, in meters.
    pub height_m: f64,
}

/// The prisms one extrusion rule raises, all drawn in its color and opacity.
#[derive(Debug, Clone, PartialEq)]
pub struct Buildings {
    pub prisms: Vec<Prism>,
    /// Color of lit roofs, opaque.
    pub color: Color,
    /// Opacity of the whole layer (overlapping faces don't show through).
    pub opacity: f32,
    /// Darken low walls.
    pub gradient: bool,
}

/// A flat-shaded triangle: corners as world pixels and height in world
/// pixels above zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Facet {
    pub corners: [[f64; 3]; 3],
    pub color: Color,
    /// A wall's outward direction (unit, in world pixels); a wall seen from
    /// behind is hidden by the rest of its building. `None` for roofs.
    pub outward: Option<[f64; 2]>,
    /// Index of its prism in [`Buildings::prisms`].
    pub prism: u32,
}

/// The buildings `style`'s extrusion rules raise from `tiles` (placed in
/// world pixels), one set per rule drawing at world zoom `zoom`, in style
/// order.
pub fn extrude(
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    zoom: f64,
) -> Vec<Buildings> {
    extrude_within(tiles, style, zoom, None)
}

/// [`extrude`], keeping only what lies inside `within` (world pixels
/// `[left, top, right, bottom]`): a building crossing its edge is cut there.
pub fn extrude_within(
    tiles: &[(TilePlacement, &VectorTile)],
    style: &MapStyle,
    zoom: f64,
    within: Option<[f64; 4]>,
) -> Vec<Buildings> {
    let maplibre_zoom = style.maplibre_zoom(zoom);
    let mut out = Vec::new();
    for rule in &style.rules {
        let (Some(extrusion), Paint::Fill(paint)) = (&rule.extrude, rule.paint) else {
            continue;
        };
        if !rule.draws_at(maplibre_zoom) {
            continue;
        }
        let color = rule.colors.as_ref().map_or(paint, |c| c.at(maplibre_zoom));
        let opacity = color.alpha()
            * rule
                .opacity
                .as_ref()
                .map_or(1.0, |c| c.at(maplibre_zoom) as f32);
        if opacity <= 0.0 {
            continue;
        }
        let mut prisms = Vec::new();
        for (placement, tile) in tiles {
            let Some(source) = tile.layer(&rule.layer) else {
                continue;
            };
            // The part of the tile kept, in tile-normalized units.
            let mut keep = [0.0, 0.0, 1.0, 1.0];
            if let Some([left, top, right, bottom]) = within {
                let at = |v: f64, start: f64| (v - start) / placement.size;
                keep = [
                    at(left, placement.dx).max(0.0),
                    at(top, placement.dy).max(0.0),
                    at(right, placement.dx).min(1.0),
                    at(bottom, placement.dy).min(1.0),
                ];
                if keep[0] >= keep[2] || keep[1] >= keep[3] {
                    continue;
                }
            }
            let extent = source.extent.max(1) as f64;
            for feature in &source.features {
                if feature.geom_type != GeomType::Polygon || !rule.matches(source, feature) {
                    continue;
                }
                let height_m = extrusion.height.of(source, feature);
                let base_m = extrusion.base.of(source, feature).min(height_m);
                let parts = feature.parts();
                let (grouped, reversed) = polygons(&parts);
                for rings in grouped {
                    let rings = rings.iter().enumerate().map(|(r, ring)| {
                        let mut normal: Vec<(f64, f64)> = ring
                            .iter()
                            .map(|&(x, y)| (x as f64 / extent, y as f64 / extent))
                            .collect();
                        if reversed {
                            normal.reverse();
                        }
                        let inside = normal.iter().all(|&(x, y)| {
                            (keep[0]..=keep[2]).contains(&x) && (keep[1]..=keep[3]).contains(&y)
                        });
                        if inside {
                            let none = vec![false; normal.len()];
                            (normal, none, Vec::new())
                        } else {
                            let clipped = clip_to_rect(&normal, keep);
                            let (on_cut, walls) = cut_walls(&clipped, &normal, keep, r > 0);
                            (clipped, on_cut, walls)
                        }
                    });
                    let mut prism = Prism {
                        rings: Vec::new(),
                        on_cut: Vec::new(),
                        cut_walls: Vec::new(),
                        base_m,
                        height_m,
                    };
                    let world = |(x, y): (f64, f64)| {
                        (
                            placement.dx + x * placement.size,
                            placement.dy + y * placement.size,
                        )
                    };
                    for (i, (ring, on_cut, walls)) in rings.enumerate() {
                        if ring.len() < 3 {
                            if i == 0 {
                                break;
                            }
                            continue;
                        }
                        prism.on_cut.push(on_cut);
                        prism
                            .cut_walls
                            .extend(walls.into_iter().map(|(a, b)| (world(a), world(b))));
                        prism.rings.push(ring.into_iter().map(world).collect());
                    }
                    if !prism.rings.is_empty() {
                        prisms.push(prism);
                    }
                }
            }
        }
        out.push(Buildings {
            prisms,
            color: Color::from_rgba(color.red(), color.green(), color.blue(), 1.0)
                .unwrap_or(Color::BLACK),
            opacity: opacity.min(1.0),
            gradient: extrusion.gradient,
        });
    }
    out
}

/// Group MVT polygon rings into polygons: each exterior ring followed by
/// the interior rings after it. As in MapLibre, exteriors wind like the
/// first ring with an area (clockwise with y down in valid tiles); whether
/// they wind the other way is returned too.
#[allow(clippy::type_complexity)]
fn polygons(parts: &[Vec<(i32, i32)>]) -> (Vec<Vec<&Vec<(i32, i32)>>>, bool) {
    let mut out: Vec<Vec<&Vec<(i32, i32)>>> = Vec::new();
    let mut exterior = 0.0f64;
    for ring in parts {
        let area = signed_area(ring.iter().map(|&(x, y)| (x as f64, y as f64)));
        if area == 0.0 {
            continue;
        }
        if exterior == 0.0 {
            exterior = area.signum();
        }
        if area.signum() == exterior {
            out.push(vec![ring]);
        } else if let Some(polygon) = out.last_mut() {
            polygon.push(ring);
        }
    }
    (out, exterior < 0.0)
}

/// Which edges of `clipped` (`original` clipped to `keep`, tile-normalized)
/// run along the cut, and the parts of them that are walls: split where
/// `original` crosses the cut, the parts with the building on their inner
/// side (the ring's inside, or for a `hole` its outside).
#[allow(clippy::type_complexity)]
fn cut_walls(
    clipped: &[(f64, f64)],
    original: &[(f64, f64)],
    keep: [f64; 4],
    hole: bool,
) -> (Vec<bool>, Vec<((f64, f64), (f64, f64))>) {
    const NUDGE: f64 = 1e-7;
    let near = |u: f64, v: f64| (u - v).abs() <= 1e-9;
    // The cut line an edge runs along: its axis and value.
    let line = |a: (f64, f64), b: (f64, f64)| {
        [(0, keep[0]), (0, keep[2]), (1, keep[1]), (1, keep[3])]
            .into_iter()
            .find(|&(axis, v)| {
                let c = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
                near(c(a), v) && near(c(b), v)
            })
    };
    let mut flags = Vec::with_capacity(clipped.len());
    let mut walls = Vec::new();
    for i in 0..clipped.len() {
        let (a, b) = (clipped[i], clipped[(i + 1) % clipped.len()]);
        let Some((axis, value)) = line(a, b) else {
            flags.push(false);
            continue;
        };
        flags.push(true);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx.hypot(dy);
        if len == 0.0 {
            continue;
        }
        // Where the original ring crosses the cut line, along the edge.
        let along = |p: (f64, f64)| ((p.0 - a.0) * dx + (p.1 - a.1) * dy) / (len * len);
        let mut stops = vec![0.0, 1.0];
        for (k, &p) in original.iter().enumerate() {
            let q = original[(k + 1) % original.len()];
            let (cp, cq) = if axis == 0 { (p.0, q.0) } else { (p.1, q.1) };
            if (cp - value) * (cq - value) <= 0.0 && cp != cq {
                let t = (value - cp) / (cq - cp);
                let cross = (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
                let s = along(cross);
                if s > 0.0 && s < 1.0 {
                    stops.push(s);
                }
            }
        }
        stops.sort_by(f64::total_cmp);
        let at = |s: f64| (a.0 + dx * s, a.1 + dy * s);
        for pair in stops.windows(2) {
            let (s0, s1) = (pair[0], pair[1]);
            if s1 - s0 <= 1e-12 {
                continue;
            }
            let middle = at((s0 + s1) / 2.0);
            // Just off the part's middle, on the inner side.
            let probe = (middle.0 - dy / len * NUDGE, middle.1 + dx / len * NUDGE);
            if contains(original, probe) != hole {
                walls.push((at(s0), at(s1)));
            }
        }
    }
    (flags, walls)
}

/// Whether `ring` holds `point` (even-odd rule).
fn contains(ring: &[(f64, f64)], (x, y): (f64, f64)) -> bool {
    let mut inside = false;
    for (i, &a) in ring.iter().enumerate() {
        let b = ring[(i + 1) % ring.len()];
        if (a.1 > y) != (b.1 > y) && x < a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1) {
            inside = !inside;
        }
    }
    inside
}

/// Twice the signed area of a ring (positive when clockwise with y down).
fn signed_area(ring: impl Iterator<Item = (f64, f64)> + Clone) -> f64 {
    let next = ring.clone().cycle().skip(1);
    ring.zip(next).map(|(a, b)| a.0 * b.1 - b.0 * a.1).sum()
}

/// MapLibre's default light at bearing 0, kept on the map (x east, y south,
/// z up), as MapLibre's shader dots it with a wall's inward normal: walls
/// facing south-southwest (its `position` azimuth of 210°) are lit most.
const LIGHT: [f64; 3] = [0.2875, -0.4980, 0.9959];
/// MapLibre's default light intensity.
const LIGHT_INTENSITY: f64 = 0.5;

impl Buildings {
    /// The triangles of every prism (placed in world pixels at
    /// `world_zoom`), each standing at the lowest height `ground` gives
    /// (world pixels, at a world point) under its outline.
    pub fn facets(&self, world_zoom: f64, ground: impl Fn(f64, f64) -> f64) -> Vec<Facet> {
        let mut out = Vec::new();
        let mut earcut = Earcut::new();
        let mut flat: Vec<[f64; 2]> = Vec::new();
        let mut holes: Vec<u32> = Vec::new();
        let mut triangles: Vec<u32> = Vec::new();
        let roof = self.shade([0.0, 0.0, 1.0], 1.0);
        for (index, prism) in self.prisms.iter().enumerate() {
            let index = index as u32;
            let outline = &prism.rings[0];
            let floor = outline
                .iter()
                .map(|&(x, y)| ground(x, y))
                .fold(f64::INFINITY, f64::min);
            let per_m = px_per_meter(outline[0].1, world_zoom);
            let top = floor + prism.height_m * per_m;
            let foot = floor + prism.base_m * per_m;
            let wall = self.wall_light(prism);
            if prism.height_m > prism.base_m {
                let edges = prism.rings.iter().enumerate().flat_map(|(r, ring)| {
                    let on_cut = prism.on_cut.get(r);
                    ring.iter().enumerate().filter_map(move |(i, &a)| {
                        let cut = on_cut.and_then(|c| c.get(i)).copied().unwrap_or(false);
                        (!cut).then(|| (a, ring[(i + 1) % ring.len()]))
                    })
                });
                for (a, b) in edges.chain(prism.cut_walls.iter().copied()) {
                    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
                    let len = dx.hypot(dy);
                    if len == 0.0 {
                        continue;
                    }
                    let outward = [dy / len, -dx / len];
                    // MapLibre's walls face inward, and those facing due
                    // east or west skip the gradient.
                    let gradient = if outward[1] == 0.0 { 1.0 } else { wall };
                    let color = self.shade([-outward[0], -outward[1], 0.0], gradient);
                    let corner = |p: (f64, f64), h: f64| [p.0, p.1, h];
                    let facet = |corners| Facet {
                        corners,
                        color,
                        outward: Some(outward),
                        prism: index,
                    };
                    out.push(facet([corner(a, foot), corner(b, foot), corner(b, top)]));
                    out.push(facet([corner(a, foot), corner(b, top), corner(a, top)]));
                }
            }
            flat.clear();
            holes.clear();
            for ring in &prism.rings {
                if !flat.is_empty() {
                    holes.push(flat.len() as u32);
                }
                flat.extend(ring.iter().map(|&(x, y)| [x, y]));
            }
            earcut.earcut(flat.iter().copied(), &holes, &mut triangles);
            let (corners, _) = triangles.as_chunks::<3>();
            for tri in corners {
                let corner = |i: u32| {
                    let [x, y] = flat[i as usize];
                    [x, y, top]
                };
                out.push(Facet {
                    corners: tri.map(corner),
                    color: roof,
                    outward: None,
                    prism: index,
                });
            }
        }
        out
    }

    /// How much the vertical gradient leaves of the light on `prism`'s
    /// walls: MapLibre's factor at their foot and top, averaged.
    fn wall_light(&self, prism: &Prism) -> f64 {
        if !self.gradient {
            return 1.0;
        }
        let low = 0.7 + (0.98 - 0.7) * (1.0 - LIGHT_INTENSITY);
        let scale = (prism.height_m / 150.0).sqrt();
        let at = |t: f64| ((t + prism.base_m) * scale).clamp(low, 1.0);
        (at(0.0) + at(1.0)) / 2.0
    }

    /// The layer color on a face with unit `normal`, lit as MapLibre lights
    /// extrusions, times `factor`.
    fn shade(&self, normal: [f64; 3], factor: f64) -> Color {
        let c = self.color;
        let (r, g, b) = (c.red() as f64, c.green() as f64, c.blue() as f64);
        let value = 0.2126 * r + 0.7152 * g + 0.0722 * b;
        let facing = (0..3)
            .map(|i| normal[i] * LIGHT[i])
            .sum::<f64>()
            .clamp(0.0, 1.0);
        let bright = (1.0 - value + LIGHT_INTENSITY).max(1.0);
        let lit = ((1.0 - LIGHT_INTENSITY) + (bright - (1.0 - LIGHT_INTENSITY)) * facing) * factor;
        // A slight ambient light keeps dark colors from going black.
        let channel = |v: f64| ((v + 0.03) * lit).clamp(0.0, 1.0) as f32;
        Color::from_rgba(channel(r), channel(g), channel(b), 1.0).unwrap_or(c)
    }
}

/// World pixels per meter at world `y` and `zoom`.
fn px_per_meter(y: f64, zoom: f64) -> f64 {
    mercator::px_per_meter(pixel_to_lat(y, zoom), zoom)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tiles::TileId;
    use crate::vector::mvt::Value;
    use crate::vector::style::Rule;

    /// A building layer with one square from `lo` to `hi` (tile units of
    /// 4096) that is 20 m tall on a 5 m base.
    fn square(lo: i32, hi: i32) -> VectorTile {
        let props = vec![
            ("render_height", Value::Int(20)),
            ("render_min_height", Value::Float(5.0)),
        ];
        VectorTile {
            layers: vec![Layer::test_square_with("building", lo, hi, props)],
        }
    }

    fn style(gradient: bool) -> MapStyle {
        let mut rule = Rule::new(
            "building",
            &[],
            Paint::Fill(Color::from_rgba8(200, 200, 200, 255)),
        );
        rule.extrude = Some(Extrusion {
            height: Height::Property("render_height".into()),
            base: Height::Property("render_min_height".into()),
            gradient,
        });
        rule.opacity = Some(crate::vector::expr::Curve::constant(0.5));
        MapStyle {
            background: Color::BLACK,
            rules: vec![rule],
            pixel_ratio: 1.0,
            maplibre_tiles: false,
        }
    }

    fn placement() -> TilePlacement {
        TilePlacement {
            id: TileId {
                zoom: 16,
                x: 0,
                y: 0,
            },
            dx: 1000.0,
            dy: 2000.0,
            size: 256.0,
        }
    }

    #[test]
    fn squares_rise_into_boxes_with_heights_from_properties() {
        let tile = square(1024, 3072);
        let buildings = extrude(&[(placement(), &tile)], &style(true), 16.0);
        assert_eq!(buildings.len(), 1);
        assert_eq!(buildings[0].opacity, 0.5);
        let prism = &buildings[0].prisms[0];
        assert_eq!((prism.base_m, prism.height_m), (5.0, 20.0));
        assert_eq!(prism.rings[0][0], (1064.0, 2064.0));
        let facets = buildings[0].facets(16.0, |_, _| 3.0);
        // Four walls of two triangles each, and a roof of two.
        assert_eq!(facets.len(), 10);
        let per_m = px_per_meter(2064.0, 16.0);
        let heights: Vec<f64> = facets
            .iter()
            .flat_map(|f| f.corners.map(|c| c[2]))
            .collect();
        let near = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(heights.iter().any(|&h| near(h, 3.0 + 5.0 * per_m)));
        assert!(heights.iter().any(|&h| near(h, 3.0 + 20.0 * per_m)));
        assert!(heights.iter().all(|&h| h >= 3.0 + 5.0 * per_m - 1e-9));
        let roofs = facets.iter().filter(|f| f.outward.is_none()).count();
        assert_eq!(roofs, 2);
    }

    #[test]
    fn tile_cuts_keep_their_walls() {
        // Reaching past the right and bottom edges: cut there, with walls
        // along the cut in case the other piece stands lower.
        let tile = square(2048, 6000);
        let buildings = extrude(&[(placement(), &tile)], &style(true), 16.0);
        assert_eq!(buildings[0].prisms[0].rings[0].len(), 4);
        let facets = buildings[0].facets(16.0, |_, _| 0.0);
        assert_eq!(facets.iter().filter(|f| f.outward.is_some()).count(), 8);
    }

    #[test]
    fn rings_wound_the_other_way_still_rise() {
        let parts = square(1024, 3072).layers[0].features[0].parts();
        assert_eq!(polygons(&parts).0.len(), 1);
        assert!(!polygons(&parts).1);
        let reversed: Vec<Vec<(i32, i32)>> = parts
            .into_iter()
            .map(|mut r| {
                r.reverse();
                r
            })
            .collect();
        let (grouped, wound) = polygons(&reversed);
        assert_eq!((grouped.len(), wound), (1, true));
    }

    #[test]
    fn roofs_leave_their_holes_open() {
        let mut buildings = extrude(&[(placement(), &square(0, 4096))], &style(false), 16.0);
        let prism = &mut buildings[0].prisms[0];
        prism.rings[0] = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        prism
            .rings
            .push(vec![(2.0, 2.0), (2.0, 8.0), (8.0, 8.0), (8.0, 2.0)]);
        let facets = buildings[0].facets(16.0, |_, _| 0.0);
        let roof: f64 = facets
            .iter()
            .filter(|f| f.outward.is_none())
            .map(|f| {
                let [a, b, c] = f.corners;
                ((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])).abs() / 2.0
            })
            .sum();
        assert!((roof - 64.0).abs() < 1e-9, "{roof}");
        // The hole's walls face into it.
        let into_hole = facets
            .iter()
            .filter_map(|f| f.outward.map(|n| (f.corners[0], n)))
            .filter(|([x, _, _], n)| *x == 2.0 && n[0] > 0.0)
            .count();
        assert_eq!(into_hole, 2);
    }

    #[test]
    fn buildings_keep_to_the_area_asked_for() {
        // The tile spans x 1000..1256; keep its left half.
        let tile = square(0, 4096);
        let within = Some([0.0, 0.0, 1128.0, 1e9]);
        let buildings = extrude_within(&[(placement(), &tile)], &style(true), 16.0, within);
        let ring = &buildings[0].prisms[0].rings[0];
        assert!(
            ring.iter().all(|p| (1000.0..=1128.0).contains(&p.0)),
            "{ring:?}"
        );
        let outside = Some([0.0, 0.0, 900.0, 1e9]);
        let none = extrude_within(&[(placement(), &tile)], &style(true), 16.0, outside);
        assert!(none[0].prisms.is_empty());
    }

    #[test]
    fn concave_buildings_cut_in_two_get_no_bridge_walls() {
        // A U opening downward: arms at x 0..1 and 3..4 (of 4), joined
        // along the top (y 0..1), each arm down to y 4; cut at y 2.
        let u = [
            (0.0, 0.0),
            (4.0, 0.0),
            (4.0, 4.0),
            (3.0, 4.0),
            (3.0, 1.0),
            (1.0, 1.0),
            (1.0, 4.0),
            (0.0, 4.0),
        ];
        let original: Vec<(f64, f64)> = u.iter().map(|&(x, y)| (x / 4.0, y / 4.0)).collect();
        let keep = [0.0, 0.5, 1.0, 1.0];
        let clipped = clip_to_rect(&original, keep);
        let (on_cut, walls) = cut_walls(&clipped, &original, keep, false);
        // Edges along the tile's sides, the original's too, go through the
        // split; the original's keep their walls.
        assert_eq!(on_cut.iter().filter(|&&c| c).count(), 6, "{clipped:?}");
        let length: f64 = walls
            .iter()
            .map(|(a, b)| (b.0 - a.0).hypot(b.1 - a.1))
            .sum();
        assert!((length - (0.5 + 0.25 + 0.25 + 0.5 + 0.25 + 0.25)).abs() < 1e-9);
        // Along the cut, only across the two arms, not the gap between them.
        let cut: Vec<_> = walls
            .iter()
            .filter(|(a, b)| a.1 == 0.5 && b.1 == 0.5)
            .collect();
        let mut spans: Vec<(f64, f64)> = cut
            .iter()
            .map(|(a, b)| (a.0.min(b.0), a.0.max(b.0)))
            .collect();
        spans.sort_by(|a, b| a.0.total_cmp(&b.0));
        assert_eq!(spans, vec![(0.0, 0.25), (0.75, 1.0)], "{walls:?}");
        // Each faces away from its arm, up toward the cut-off part.
        for (a, b) in cut {
            let outward_y = -(b.0 - a.0);
            assert!(outward_y < 0.0, "{a:?} {b:?}");
        }
    }

    #[test]
    fn rules_draw_only_in_their_zooms() {
        let tile = square(1024, 3072);
        let mut style = style(true);
        style.rules[0].zooms = Some((14.0, f64::INFINITY));
        assert!(extrude(&[(placement(), &tile)], &style, 14.5).is_empty());
        assert_eq!(extrude(&[(placement(), &tile)], &style, 15.0).len(), 1);
    }

    #[test]
    fn holes_follow_their_outline() {
        let outer = vec![(0, 0), (10, 0), (10, 10), (0, 10)];
        let hole = vec![(2, 2), (2, 8), (8, 8), (8, 2)];
        let other = vec![(20, 0), (30, 0), (30, 10), (20, 10)];
        let parts = [outer.clone(), hole.clone(), other.clone()];
        let grouped = polygons(&parts);
        let (first, _) = &grouped;
        assert_eq!(first.len(), 2);
        assert_eq!(grouped, (vec![vec![&outer, &hole], vec![&other]], false));
    }

    #[test]
    fn walls_facing_the_light_are_brighter() {
        let buildings = &extrude(&[(placement(), &square(1024, 3072))], &style(false), 16.0)[0];
        let lum = |c: Color| c.red() + c.green() + c.blue();
        // The light comes from the south-southwest and above; walls are
        // shaded by their inward normal, as in MapLibre.
        let facing_south = buildings.shade([0.0, -1.0, 0.0], 1.0);
        let facing_north = buildings.shade([0.0, 1.0, 0.0], 1.0);
        let roof = buildings.shade([0.0, 0.0, 1.0], 1.0);
        assert!(lum(roof) > lum(facing_south) && lum(facing_south) > lum(facing_north));
        let facets = buildings.facets(16.0, |_, _| 0.0);
        let wall = |facing: f64| {
            facets
                .iter()
                .find(|f| f.outward.is_some_and(|n| n[1] == facing))
                .map(|f| lum(f.color))
                .unwrap()
        };
        assert!(wall(1.0) > wall(-1.0), "south walls are the bright ones");
        // MapLibre's gradient darkens walls standing on the ground, not
        // ones raised on a base.
        let shaded = &extrude(&[(placement(), &square(1024, 3072))], &style(true), 16.0)[0];
        let mut grounded = shaded.prisms[0].clone();
        assert_eq!(shaded.wall_light(&grounded), 1.0);
        grounded.base_m = 0.0;
        assert!(shaded.wall_light(&grounded) < 0.9);
        assert_eq!(buildings.wall_light(&grounded), 1.0);
    }

    #[test]
    fn heights_parse_and_round_trip() {
        let json = serde_json::json!({"height": "render_height", "base": 3, "gradient": false});
        let extrusion = Extrusion::parse(&json).unwrap();
        assert_eq!(extrusion.height, Height::Property("render_height".into()));
        assert_eq!(extrusion.base, Height::Meters(3.0));
        assert_eq!(Extrusion::parse(&extrusion.to_json()).unwrap(), extrusion);
        assert!(Extrusion::parse(&serde_json::json!({"height": -1})).is_err());
        assert!(Extrusion::parse(&serde_json::json!({"top": 1})).is_err());
        let defaults = Extrusion::parse(&serde_json::json!({})).unwrap();
        assert_eq!(
            (defaults.height, defaults.gradient),
            (Height::Meters(0.0), true)
        );
    }
}
