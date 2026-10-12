//! Software rasterizer for the terrain mesh: fills screen-space triangles
//! with an affinely mapped, mip-blended texture and records their depth.

use tiny_skia::PremultipliedColorU8;

use crate::render::sample::{lerp, Image};

/// Subpixel steps per pixel that triangle vertices snap to.
const SUB: f64 = 256.0;
/// Largest snapped coordinate (a million pixels off screen); triangles
/// reaching farther are near-plane slivers and are skipped.
const MAX_SNAPPED: f64 = 1e6 * SUB;

type Vertex = ((f64, f64), (f64, f64), f64);

/// Fill the pixels whose centers lie in a triangle's screen points with its
/// texture, mapped affinely and sampled bilinearly (edges clamp), blended
/// over what is there. Where the triangle shrinks the texture, the two mip
/// levels (`textures`, finest first) nearest its scale are blended. Shared
/// edges follow a top-left rule and are evaluated identically by both
/// triangles, so neighbours neither overlap nor leave gaps along them.
pub(super) fn fill_triangle(
    pixels: &mut [PremultipliedColorU8],
    depth: &mut [f32],
    (width, height): (usize, usize),
    tri: &[Vertex; 3],
    textures: &[Image],
) {
    let [(t0, s0, d0), (t1, s1, d1), (t2, s2, d2)] = *tri;
    let inverse = [1.0 / d0, 1.0 / d1, 1.0 / d2];
    let Some(coverage) = Coverage::new([s0, s1, s2]) else {
        return;
    };
    let min_x = s0.0.min(s1.0).min(s2.0).floor().max(0.0);
    let max_x = s0.0.max(s1.0).max(s2.0).ceil().min(width as f64);
    let min_y = s0.1.min(s1.1).min(s2.1).floor().max(0.0);
    let max_y = s0.1.max(s1.1).max(s2.1).ceil().min(height as f64);
    if min_x >= max_x || min_y >= max_y {
        return;
    }
    // Texture coordinates are affine in the pixel center (barycentric
    // weights of the unsnapped vertices).
    let norm = (s1.0 - s0.0) * (s2.1 - s0.1) - (s1.1 - s0.1) * (s2.0 - s0.0);
    let weights = |x: f64, y: f64| {
        let w = |p: (f64, f64), q: (f64, f64)| {
            ((q.0 - p.0) * (y - p.1) - (q.1 - p.1) * (x - p.0)) / norm
        };
        [w(s1, s2), w(s2, s0), w(s0, s1)]
    };
    let at = |w: [f64; 3]| {
        (
            w[0] * t0.0 + w[1] * t1.0 + w[2] * t2.0,
            w[0] * t0.1 + w[1] * t1.1 + w[2] * t2.1,
        )
    };
    let base = at(weights(0.5, 0.5));
    let (across, down) = (at(weights(1.5, 0.5)), at(weights(0.5, 1.5)));
    let step = (across.0 - base.0, across.1 - base.1);
    let texels = |p: (f64, f64)| (p.0 - base.0).powi(2) + (p.1 - base.1).powi(2);
    let lod = 0.5 * texels(across).max(texels(down)).log2();
    let (fine, coarse) = mip_levels(lod, textures.len().saturating_sub(1));
    let Some(texture) = textures.get(fine) else {
        return;
    };
    let mut fine = Walk::new(texture, fine, step);
    let mut coarse =
        coarse.map(|(level, weight)| (Walk::new(&textures[level], level, step), weight));
    let near = |w: [f64; 3]| w[0] * inverse[0] + w[1] * inverse[1] + w[2] * inverse[2];
    let step_near = near(weights(1.5, 0.5)) - near(weights(0.5, 0.5));
    for y in min_y as usize..max_y as usize {
        let Some((x0, x1)) = coverage.row(y, min_x, max_x) else {
            continue;
        };
        let first = weights(x0 as f64 + 0.5, y as f64 + 0.5);
        fine.start(at(first));
        if let Some((walk, _)) = &mut coarse {
            walk.start(at(first));
        }
        let mut inv = near(first);
        let row = y * width + x0..=y * width + x1;
        for d in &mut depth[row.clone()] {
            *d = inv as f32;
            inv += step_near;
        }
        for px in &mut pixels[row] {
            let texel = match &mut coarse {
                Some((walk, weight)) => lerp(fine.next(), walk.next(), *weight),
                None => fine.next(),
            };
            *px = over(texel, *px);
        }
    }
}

/// The mip level to sample at level of detail `lod` (of `0..=last`), and
/// the next coarser one with its blend weight (`0..256`) when between two.
fn mip_levels(lod: f64, last: usize) -> (usize, Option<(usize, u32)>) {
    match lod {
        lod if lod > 0.0 && (lod as usize) < last => {
            let level = lod as usize;
            let weight = ((lod - level as f64) * 256.0) as u32;
            (level, Some((level + 1, weight)))
        }
        lod if lod > 0.0 => (last, None),
        _ => (0, None),
    }
}

/// Which pixel centers a triangle covers, decided exactly on its vertices
/// snapped to 1/[`SUB`] pixel.
///
/// Integer edge functions make every pixel center along a shared edge or at
/// a shared vertex belong to exactly one triangle.
struct Coverage {
    /// Edge `i`, opposite vertex `i`, as `(a, b, c, owns)`: `a·x + b·y + c ≥
    /// 0` inside, and whether centers exactly on it are inside.
    edges: [(i128, i128, i128, bool); 3],
}

impl Coverage {
    /// The coverage of the triangle with these screen points, or `None` when
    /// it is degenerate or reaches too far off screen.
    fn new([s0, s1, s2]: [(f64, f64); 3]) -> Option<Self> {
        let snap = |p: (f64, f64)| -> Option<(i128, i128)> {
            let (x, y) = (p.0 * SUB, p.1 * SUB);
            (x.abs() <= MAX_SNAPPED && y.abs() <= MAX_SNAPPED)
                .then(|| (x.round() as i128, y.round() as i128))
        };
        let (p0, p1, p2) = (snap(s0)?, snap(s1)?, snap(s2)?);
        let area = (p1.0 - p0.0) * (p2.1 - p0.1) - (p1.1 - p0.1) * (p2.0 - p0.0);
        if area == 0 {
            return None;
        }
        let orient = area.signum();
        // Edge i is opposite vertex i: e(p) = a·x + b·y + c ≥ 0 inside. Taking the
        // endpoints in a fixed order gives both triangles of an edge the same
        // numbers up to sign.
        let edge = |p: (i128, i128), q: (i128, i128)| {
            let (first, second, flip) = if p <= q {
                (p, q, orient)
            } else {
                (q, p, -orient)
            };
            let a = (first.1 - second.1) * flip;
            let b = (second.0 - first.0) * flip;
            let c = (first.0 * second.1 - first.1 * second.0) * flip;
            // Top-left rule: pixels exactly on a top or left edge are inside.
            let owns = a > 0 || (a == 0 && b > 0);
            (a, b, c, owns)
        };
        Some(Self {
            edges: [edge(p1, p2), edge(p2, p0), edge(p0, p1)],
        })
    }

    /// The first and last pixel of row `y`, within `min_x..max_x`, whose
    /// centers are covered, or `None` when none is.
    fn row(&self, y: usize, min_x: f64, max_x: f64) -> Option<(usize, usize)> {
        let half = SUB as i128 / 2;
        let cy = y as i128 * SUB as i128 + half;
        // Each edge bounds the row's pixel centers from one side; keep the
        // first and last integer x whose center passes all three.
        let (mut lo, mut hi) = (min_x as i128, max_x as i128 - 1);
        for &(a, b, c, owns) in &self.edges {
            let rest = b * cy + c;
            if a == 0 {
                if rest < 0 || (rest == 0 && !owns) {
                    lo = i128::MAX;
                }
                continue;
            }
            // Center x is inside when a·(SUB·x + SUB/2) + rest ≥ 0 (> 0 off
            // the owned edges), i.e. x against n / d:
            let (n, d) = (-rest - a * half, a * SUB as i128);
            if a > 0 {
                lo = lo.max(if owns {
                    ceil_div(n, d)
                } else {
                    n.div_euclid(d) + 1
                });
            } else {
                // Dividing by a negative d flips the inequality.
                let (n, d) = (-n, -d);
                hi = hi.min(if owns {
                    n.div_euclid(d)
                } else {
                    ceil_div(n, d) - 1
                });
            }
        }
        (lo <= hi).then_some((lo as usize, hi as usize))
    }
}

/// Texture positions step in 1/65536 texels along a row.
const ONE: f64 = 65536.0;

/// One mip level of a texture, sampled pixel by pixel along a row.
struct Walk<'a> {
    image: &'a Image<'a>,
    /// Texels of this level per finest-level texel.
    scale: f64,
    /// Texels moved per pixel, in 1/[`ONE`] texels.
    step: (i64, i64),
    max: (i64, i64),
    at: (i64, i64),
}

impl<'a> Walk<'a> {
    /// Mip `level` of a texture (`image`), moving `step` finest-level texels
    /// per pixel.
    fn new(image: &'a Image<'a>, level: usize, step: (f64, f64)) -> Self {
        let scale = 0.5f64.powi(level as i32);
        Self {
            image,
            scale,
            step: ((step.0 * scale * ONE) as i64, (step.1 * scale * ONE) as i64),
            max: ((image.max_x * ONE) as i64, (image.max_y * ONE) as i64),
            at: (0, 0),
        }
    }

    /// Start a row at finest-level texture position `p` (texel corners on
    /// whole numbers).
    fn start(&mut self, p: (f64, f64)) {
        let texel = |v: f64| ((v * self.scale - 0.5) * ONE) as i64;
        self.at = (texel(p.0), texel(p.1));
    }

    /// The texture at the current pixel, then step to the next.
    fn next(&mut self) -> PremultipliedColorU8 {
        let sx = ((self.at.0.clamp(0, self.max.0) + 128) >> 8) as usize;
        let sy = ((self.at.1.clamp(0, self.max.1) + 128) >> 8) as usize;
        self.at = (
            self.at.0.wrapping_add(self.step.0),
            self.at.1.wrapping_add(self.step.1),
        );
        self.image.sample_fixed(sx, sy)
    }
}

/// `n / d` rounded up, for `d > 0`.
fn ceil_div(n: i128, d: i128) -> i128 {
    -(-n).div_euclid(d)
}

/// `src` drawn over `dst`, both premultiplied.
fn over(src: PremultipliedColorU8, dst: PremultipliedColorU8) -> PremultipliedColorU8 {
    match src.alpha() {
        0xff => src,
        0 => dst,
        a => {
            let keep = 255 - a as u32;
            let mix = |s: u8, d: u8| (s as u32 + (d as u32 * keep + 127) / 255).min(255) as u8;
            PremultipliedColorU8::from_rgba(
                mix(src.red(), dst.red()),
                mix(src.green(), dst.green()),
                mix(src.blue(), dst.blue()),
                mix(src.alpha(), dst.alpha()),
            )
            .unwrap_or(src)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::clip::contains;
    use crate::render::sample::mip_chain;
    use tiny_skia::Pixmap;

    /// Fill a grid of `n`×`n` cells (two triangles each) whose corners come
    /// from `corner`, with a half-transparent texture, and count pixels
    /// drawn more than once and pixel centers inside the grid left empty.
    fn seams(n: usize, corner: impl Fn(usize, usize) -> (f64, f64)) -> (usize, usize) {
        let mut texture = Pixmap::new(4, 4).unwrap();
        texture.fill(tiny_skia::Color::from_rgba8(255, 0, 0, 128));
        let image = Image::new(&texture);
        let mut target = Pixmap::new(200, 200).unwrap();
        let size = (200, 200);
        let v = |c: usize, r: usize| ((c as f64, r as f64), corner(c, r), 0.0);
        for r in 0..n {
            for c in 0..n {
                let q = [v(c, r), v(c + 1, r), v(c + 1, r + 1), v(c, r + 1)];
                fill_triangle(
                    target.pixels_mut(),
                    &mut vec![0.0; 200 * 200],
                    size,
                    &[q[0], q[1], q[2]],
                    std::slice::from_ref(&image),
                );
                fill_triangle(
                    target.pixels_mut(),
                    &mut vec![0.0; 200 * 200],
                    size,
                    &[q[0], q[2], q[3]],
                    std::slice::from_ref(&image),
                );
            }
        }
        let single = target.pixels().iter().map(|p| p.alpha()).max().unwrap_or(0);
        let doubled = target.pixels().iter().filter(|p| p.alpha() > 128).count();
        // Pixel centers strictly inside the grid's outline must be drawn.
        let outline: Vec<(f32, f32)> = (0..n)
            .map(|i| corner(i, 0))
            .chain((0..n).map(|i| corner(n, i)))
            .chain((0..n).map(|i| corner(n - i, n)))
            .chain((0..n).map(|i| corner(0, n - i)))
            .map(|(x, y)| (x as f32, y as f32))
            .collect();
        let mut gaps = 0;
        for y in 0..200 {
            for x in 0..200 {
                let p = (x as f32 + 0.5, y as f32 + 0.5);
                let near_edge = outline
                    .iter()
                    .zip(outline.iter().cycle().skip(1))
                    .any(|(a, b)| segment_distance(p, *a, *b) < 1e-3);
                if !near_edge && contains(&outline, p) && target.pixel(x, y).unwrap().alpha() == 0 {
                    gaps += 1;
                }
            }
        }
        assert!(single <= 128 || doubled > 0);
        (doubled, gaps)
    }

    fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx * dx + dy * dy;
        let t = if len > 0.0 {
            (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
    }

    #[test]
    fn shared_edges_are_drawn_exactly_once() {
        // Corners on half pixels put pixel centers exactly on the edges.
        let snapped = |c: usize, r: usize| {
            let jitter = ((c * 7 + r * 3) % 5) as f64 * 0.5;
            (
                10.5 + c as f64 * 14.0 + jitter,
                12.5 + r as f64 * 13.0 - jitter,
            )
        };
        assert_eq!(seams(12, snapped), (0, 0));
        // A quarter turn computed in floating point leaves edges a hair off
        // the pixel-center rows.
        let (sin, cos) = std::f64::consts::FRAC_PI_2.sin_cos();
        let turned = |c: usize, r: usize| {
            let (x, y) = (c as f64 * 12.0 - 66.0, r as f64 * 12.0 - 66.0);
            (100.5 + x * cos - y * sin, 100.5 + x * sin + y * cos)
        };
        assert_eq!(seams(11, turned), (0, 0));
    }

    #[test]
    fn shrunk_textures_average_instead_of_aliasing() {
        // A one-texel checkerboard squeezed 8× must come out gray, not in
        // whichever black or white texels the pixel centers hit.
        let mut texture = Pixmap::new(256, 256).unwrap();
        for (i, px) in texture.pixels_mut().iter_mut().enumerate() {
            let v = if (i % 256 + i / 256) % 2 == 0 { 255 } else { 0 };
            *px = PremultipliedColorU8::from_rgba(v, v, v, 255).unwrap();
        }
        let mips = mip_chain(&texture);
        let textures: Vec<Image> = std::iter::once(&texture)
            .chain(&mips)
            .map(Image::new)
            .collect();
        let mut target = Pixmap::new(32, 32).unwrap();
        let corner = |t: f64, s: f64| ((t + 0.5, s + 0.5), (t / 8.0, s / 8.0), 1.0);
        for tri in [
            [corner(0.0, 0.0), corner(256.0, 0.0), corner(256.0, 256.0)],
            [corner(0.0, 0.0), corner(256.0, 256.0), corner(0.0, 256.0)],
        ] {
            fill_triangle(
                target.pixels_mut(),
                &mut [0.0; 1024],
                (32, 32),
                &tri,
                &textures,
            );
        }
        let reds: Vec<u8> = target.pixels().iter().map(|p| p.red()).collect();
        assert!(reds.iter().all(|r| r.abs_diff(128) <= 8), "{reds:?}");
    }

    #[test]
    fn slivers_are_skipped() {
        let texture = Pixmap::new(4, 4).unwrap();
        let image = Image::new(&texture);
        let mut target = Pixmap::new(8, 8).unwrap();
        let sliver = [
            ((0.0, 0.0), (1.0, 1.0), 0.0),
            ((3.0, 0.0), (f64::MAX / 4.0, 1.0 + 1e-300), 0.0),
            ((0.0, 3.0), (6.0, 1.0), 0.0),
        ];
        fill_triangle(
            target.pixels_mut(),
            &mut [0.0; 64],
            (8, 8),
            &sliver,
            &[image],
        );
    }
}
