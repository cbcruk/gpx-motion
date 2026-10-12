//! Texture sampling helpers: bilinear lookups and mip chains.

use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Borrowed pixels of a texture for fast bilinear lookups.
pub(crate) struct Image<'a> {
    pixels: &'a [PremultipliedColorU8],
    width: usize,
    pub(crate) max_x: f64,
    pub(crate) max_y: f64,
}

impl<'a> Image<'a> {
    pub(crate) fn new(pixmap: &'a Pixmap) -> Self {
        Self {
            pixels: pixmap.pixels(),
            width: pixmap.width() as usize,
            max_x: f64::from(pixmap.width()) - 1.0,
            max_y: f64::from(pixmap.height()) - 1.0,
        }
    }

    /// Bilinear sample at `(fx, fy)` in 1/256 pixels (pixel centers on
    /// multiples of 256), which must lie within `0..=max * 256`.
    pub(crate) fn sample_fixed(&self, fx: usize, fy: usize) -> PremultipliedColorU8 {
        let (x0, y0) = (fx >> 8, fy >> 8);
        let (wx, wy) = ((fx & 0xff) as u32, (fy & 0xff) as u32);
        let dx = usize::from((x0 as f64) < self.max_x);
        let dy = if (y0 as f64) < self.max_y {
            self.width
        } else {
            0
        };
        let i = y0 * self.width + x0;
        let (a, b, c, d) = (
            self.pixels[i],
            self.pixels[i + dx],
            self.pixels[i + dy],
            self.pixels[i + dy + dx],
        );
        // Fixed-point weights out of 256 per axis; a convex mix of
        // premultiplied colors stays premultiplied.
        let mix = |a: u8, b: u8, c: u8, d: u8| {
            let top = u32::from(a) * (256 - wx) + u32::from(b) * wx;
            let bottom = u32::from(c) * (256 - wx) + u32::from(d) * wx;
            ((top * (256 - wy) + bottom * wy + (1 << 15)) >> 16) as u8
        };
        PremultipliedColorU8::from_rgba(
            mix(a.red(), b.red(), c.red(), d.red()),
            mix(a.green(), b.green(), c.green(), d.green()),
            mix(a.blue(), b.blue(), c.blue(), d.blue()),
            mix(a.alpha(), b.alpha(), c.alpha(), d.alpha()),
        )
        .unwrap_or(a)
    }
}

/// Each smaller mip level of `texture`: half the size of the one before
/// (rounded up), every texel the mean of the up to four it covers, down to
/// 1×1.
pub(crate) fn mip_chain(texture: &Pixmap) -> Vec<Pixmap> {
    let mut levels: Vec<Pixmap> = Vec::new();
    loop {
        let last = levels.last().unwrap_or(texture);
        if last.width() <= 1 && last.height() <= 1 {
            return levels;
        }
        let Some(next) = halve(last) else {
            return levels;
        };
        levels.push(next);
    }
}

fn halve(texture: &Pixmap) -> Option<Pixmap> {
    let (w, h) = (texture.width() as usize, texture.height() as usize);
    let (hw, hh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = Pixmap::new(hw as u32, hh as u32)?;
    let src = texture.pixels();
    for (i, px) in out.pixels_mut().iter_mut().enumerate() {
        let (x, y) = (i % hw * 2, i / hw * 2);
        let (x1, y1) = ((x + 1).min(w - 1), (y + 1).min(h - 1));
        let quad = [
            src[y * w + x],
            src[y * w + x1],
            src[y1 * w + x],
            src[y1 * w + x1],
        ];
        let mean = |f: fn(PremultipliedColorU8) -> u8| {
            ((quad.iter().map(|&p| u32::from(f(p))).sum::<u32>() + 2) / 4) as u8
        };
        *px = PremultipliedColorU8::from_rgba(
            mean(PremultipliedColorU8::red),
            mean(PremultipliedColorU8::green),
            mean(PremultipliedColorU8::blue),
            mean(PremultipliedColorU8::alpha),
        )
        .unwrap_or(quad[0]);
    }
    Some(out)
}

/// `a` mixed toward `b` by `weight` out of 256, both premultiplied.
pub(crate) fn lerp(
    a: PremultipliedColorU8,
    b: PremultipliedColorU8,
    weight: u32,
) -> PremultipliedColorU8 {
    let mix =
        |a: u8, b: u8| ((u32::from(a) * (256 - weight) + u32::from(b) * weight + 128) >> 8) as u8;
    PremultipliedColorU8::from_rgba(
        mix(a.red(), b.red()),
        mix(a.green(), b.green()),
        mix(a.blue(), b.blue()),
        mix(a.alpha(), b.alpha()),
    )
    .unwrap_or(a)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_skia::ColorU8;

    fn gray(v: u8) -> PremultipliedColorU8 {
        PremultipliedColorU8::from_rgba(v, v, v, 255).unwrap()
    }

    /// A `width`×`height` opaque gray pixmap whose texel `i` (row-major)
    /// has the value `values[i]`.
    fn grays(width: u32, height: u32, values: &[u8]) -> Pixmap {
        let mut pixmap = Pixmap::new(width, height).unwrap();
        for (px, &v) in pixmap.pixels_mut().iter_mut().zip(values) {
            *px = gray(v);
        }
        pixmap
    }

    #[test]
    fn samples_on_texel_centers_return_the_texel() {
        let pixmap = grays(2, 2, &[10, 20, 30, 40]);
        let image = Image::new(&pixmap);
        assert_eq!((image.max_x, image.max_y), (1.0, 1.0));
        assert_eq!(image.sample_fixed(0, 0), gray(10));
        assert_eq!(image.sample_fixed(256, 0), gray(20));
        assert_eq!(image.sample_fixed(0, 256), gray(30));
        assert_eq!(image.sample_fixed(256, 256), gray(40));
    }

    #[test]
    fn samples_between_texels_mix_them_bilinearly() {
        let pixmap = grays(2, 2, &[0, 200, 100, 255]);
        let image = Image::new(&pixmap);
        assert_eq!(image.sample_fixed(128, 0), gray(100));
        assert_eq!(image.sample_fixed(0, 128), gray(50));
        assert_eq!(image.sample_fixed(64, 0), gray(50));
        // (0 + 200 + 100 + 255) / 4, rounded.
        assert_eq!(image.sample_fixed(128, 128), gray(139));
    }

    #[test]
    fn samples_on_the_last_row_and_column_stay_inside() {
        let pixmap = grays(3, 1, &[0, 100, 250]);
        let image = Image::new(&pixmap);
        assert_eq!(image.sample_fixed(512, 0), gray(250));
        // On a one-row image the vertical weight has no row below to mix in.
        assert_eq!(image.sample_fixed(384, 255), gray(175));
    }

    #[test]
    fn mip_chain_halves_rounding_up_down_to_one_texel() {
        let sizes: Vec<_> = mip_chain(&Pixmap::new(5, 3).unwrap())
            .iter()
            .map(|level| (level.width(), level.height()))
            .collect();
        assert_eq!(sizes, [(3, 2), (2, 1), (1, 1)]);
        assert!(mip_chain(&Pixmap::new(1, 1).unwrap()).is_empty());
    }

    #[test]
    fn mip_texels_average_the_texels_they_cover() {
        let levels = mip_chain(&grays(3, 2, &[0, 100, 40, 200, 60, 80]));
        let first = &levels[0];
        assert_eq!((first.width(), first.height()), (2, 1));
        // Left: (0 + 100 + 200 + 60) / 4; right: the odd column is repeated.
        assert_eq!(first.pixels(), [gray(90), gray(60)]);
        assert_eq!(levels[1].pixels(), [gray(75)]);
    }

    #[test]
    fn mip_texels_keep_colors_premultiplied() {
        let mut pixmap = Pixmap::new(2, 1).unwrap();
        pixmap.pixels_mut()[0] = ColorU8::from_rgba(255, 0, 0, 255).premultiply();
        let level = &mip_chain(&pixmap)[0];
        let texel = level.pixels()[0];
        assert_eq!((texel.red(), texel.alpha()), (128, 128));
        assert!(texel.red() <= texel.alpha());
    }

    #[test]
    fn lerp_weights_out_of_256() {
        let (a, b) = (gray(0), gray(200));
        assert_eq!(lerp(a, b, 0), a);
        assert_eq!(lerp(a, b, 256), b);
        assert_eq!(lerp(a, b, 64), gray(50));
    }
}
