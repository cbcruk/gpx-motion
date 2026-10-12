//! Which map tiles to drape over the ground of a 3D view, and at which
//! zoom: finer near the camera, coarser far away, so each texel of a tile's
//! texture covers about one device pixel where it is seen.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::camera::{Camera, NEAR};
use crate::geo::mercator::TILE_SIZE;
use crate::tiles::TileId;

/// How sharply to drape the ground, and how much of it at most.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundDetail {
    /// Texels across a tile's texture.
    pub texture_px: f64,
    /// Device pixels per screen pixel the view is drawn at.
    pub pixel_ratio: f64,
    /// Most tiles in the tree, inner ones included.
    pub max_tiles: usize,
    /// Finest tile zoom.
    pub max_zoom: u32,
}

/// One tile of a [`ground_tile_tree`] tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundTile {
    pub id: TileId,
    /// Index of the tile it splits, `None` for a root.
    pub parent: Option<usize>,
    /// Whether it is split further; leaves together cover the ground once.
    pub split: bool,
}

/// Tiles covering the ground `camera` draws (out to [`Camera::far_depth`]),
/// as a quadtree with each split tile before its children.
///
/// A tile in view is split while its texels are coarser than a device pixel
/// where it comes nearest the eye, the coarsest first, until
/// `detail.max_tiles` or `detail.max_zoom` stop it. Leaves are the tiles to
/// draw; inner tiles can stand in for their children until those are ready.
///
/// Unlike [`Camera::ground_tiles`], which covers the whole view at one tile
/// zoom, this mixes zooms by distance from the eye, hence the distinct name.
pub fn ground_tile_tree(camera: &Camera, detail: &GroundDetail) -> Vec<GroundTile> {
    let zoom = camera.zoom;
    let (ex, ey, eh) = camera.eye();
    let radius = camera.far_depth();
    let (width, height) = camera.viewport();
    // World pixels across a tile at `z`, and its world rectangle.
    let size = |z: u32| TILE_SIZE * 2f64.powf(zoom - z as f64);
    let rect = |id: TileId| {
        let s = size(id.zoom);
        (id.x as f64 * s, id.y as f64 * s, s)
    };
    // Ground distance from the eye to the nearest point of a tile.
    let gap = |id: TileId| {
        let (x0, y0, s) = rect(id);
        let dx = (x0 - ex).max(ex - (x0 + s)).max(0.0);
        let dy = (y0 - ey).max(ey - (y0 + s)).max(0.0);
        dx.hypot(dy)
    };
    // Whether the tile's ground might be on screen (not wholly behind the
    // camera or past one edge).
    let in_view = |id: TileId| {
        let (x0, y0, s) = rect(id);
        let corners: Option<Vec<(f64, f64)>> =
            [(x0, y0), (x0 + s, y0), (x0, y0 + s), (x0 + s, y0 + s)]
                .iter()
                .map(|&(x, y)| {
                    let v = camera.view(x, y, 0.0);
                    (v.depth >= NEAR).then(|| camera.to_screen(v))
                })
                .collect();
        let Some(corners) = corners else {
            return [(x0, y0), (x0 + s, y0), (x0, y0 + s), (x0 + s, y0 + s)]
                .iter()
                .any(|&(x, y)| camera.view(x, y, 0.0).depth >= NEAR);
        };
        let all = |f: &dyn Fn(&(f64, f64)) -> bool| corners.iter().all(f);
        !(all(&|p| p.0 < 0.0)
            || all(&|p| p.0 > width)
            || all(&|p| p.1 < 0.0)
            || all(&|p| p.1 > height))
    };
    // How many device pixels a texel of the tile spans at most.
    let blur = |id: TileId| {
        if !in_view(id) {
            return 0.0;
        }
        let nearest = gap(id).hypot(eh);
        let texel = size(id.zoom) / detail.texture_px;
        texel / (nearest / (camera.focal() * detail.pixel_ratio))
    };

    let root_zoom = (zoom - (2.0 * radius / TILE_SIZE).log2())
        .floor()
        .clamp(0.0, detail.max_zoom as f64) as u32;
    let s = size(root_zoom);
    let last = (1i64 << root_zoom) - 1;
    let cell = |v: f64| ((v / s).floor() as i64).clamp(0, last) as u32;
    let mut tiles: Vec<GroundTile> = Vec::new();
    for y in cell(ey - radius)..=cell(ey + radius) {
        for x in cell(ex - radius)..=cell(ex + radius) {
            let id = TileId {
                zoom: root_zoom,
                x,
                y,
            };
            if gap(id) <= radius {
                tiles.push(GroundTile {
                    id,
                    parent: None,
                    split: false,
                });
            }
        }
    }
    let mut queue: BinaryHeap<Need> = (0..tiles.len())
        .map(|i| Need(blur(tiles[i].id), i))
        .collect();
    while let Some(Need(need, i)) = queue.pop() {
        let id = tiles[i].id;
        if need <= 1.0 || tiles.len() + 4 > detail.max_tiles {
            break;
        }
        if id.zoom >= detail.max_zoom {
            continue;
        }
        tiles[i].split = true;
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let child = TileId {
                zoom: id.zoom + 1,
                x: id.x * 2 + dx,
                y: id.y * 2 + dy,
            };
            if gap(child) <= radius {
                queue.push(Need(blur(child), tiles.len()));
                tiles.push(GroundTile {
                    id: child,
                    parent: Some(i),
                    split: false,
                });
            }
        }
    }
    tiles
}

/// A tile index ordered by how much it needs splitting.
struct Need(f64, usize);

impl PartialEq for Need {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for Need {}

impl PartialOrd for Need {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Need {
    fn cmp(&self, other: &Self) -> Ordering {
        self.0.total_cmp(&other.0).then(other.1.cmp(&self.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::Lens;

    fn camera() -> Camera {
        let mut cam = Camera::new(35.8, 129.2, 13.0, 640, 640, Lens::default());
        cam.distance *= 2.0;
        cam
    }

    fn detail() -> GroundDetail {
        GroundDetail {
            texture_px: 512.0,
            pixel_ratio: 2.0,
            max_tiles: 160,
            max_zoom: 18,
        }
    }

    fn leaves(tiles: &[GroundTile]) -> Vec<TileId> {
        tiles.iter().filter(|t| !t.split).map(|t| t.id).collect()
    }

    #[test]
    fn leaves_cover_the_ground_once_finest_where_the_camera_looks_closest() {
        let camera = camera();
        let tiles = ground_tile_tree(&camera, &detail());
        assert!(tiles.len() <= 160);
        for (i, t) in tiles.iter().enumerate() {
            if let Some(p) = t.parent {
                assert!(p < i && tiles[p].split);
                assert_eq!(tiles[p].id.zoom + 1, t.id.zoom);
                assert_eq!((t.id.x / 2, t.id.y / 2), (tiles[p].id.x, tiles[p].id.y));
            }
        }
        let leaves = leaves(&tiles);
        for a in &leaves {
            for b in &leaves {
                if a.zoom < b.zoom {
                    let k = b.zoom - a.zoom;
                    assert!((b.x >> k, b.y >> k) != (a.x, a.y), "{a:?} contains {b:?}");
                }
            }
        }
        let size = |z: u32| 256.0 * 2f64.powf(camera.zoom - z as f64);
        let leaf_at = |x: f64, y: f64| {
            leaves
                .iter()
                .find(|t| {
                    let s = size(t.zoom);
                    let (x0, y0) = (t.x as f64 * s, t.y as f64 * s);
                    (x0..x0 + s).contains(&x) && (y0..y0 + s).contains(&y)
                })
                .copied()
        };
        // The ground at the bottom of the screen (nearest) is sharper than
        // at the target, which is sharper than ground behind the camera.
        let (w, h) = camera.viewport();
        let near = camera.ground_at(w / 2.0, h - 1.0).unwrap();
        let (tx, ty) = camera.target();
        let (ex, ey, _) = camera.eye();
        let behind = (ex + (ex - tx), ey + (ey - ty));
        let (near, middle, back) = (
            leaf_at(near.0, near.1).unwrap(),
            leaf_at(tx, ty).unwrap(),
            leaf_at(behind.0, behind.1).unwrap(),
        );
        assert!(
            near.zoom >= middle.zoom && middle.zoom > back.zoom,
            "{near:?} {middle:?} {back:?}"
        );
        // At the target a texel is about a device pixel.
        let depth = camera.distance;
        let texel = size(middle.zoom) / 512.0;
        assert!(
            texel <= depth / camera.focal() / 2.0 * 1.01,
            "texel {texel}"
        );
    }

    #[test]
    fn the_budget_and_max_zoom_cap_the_tree() {
        let camera = camera();
        let small = ground_tile_tree(
            &camera,
            &GroundDetail {
                max_tiles: 30,
                ..detail()
            },
        );
        assert!(small.len() <= 30);
        let shallow = ground_tile_tree(
            &camera,
            &GroundDetail {
                max_zoom: 12,
                ..detail()
            },
        );
        assert!(shallow.iter().all(|t| t.id.zoom <= 12));
    }
}
