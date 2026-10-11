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
            max_x: pixmap.width() as f64 - 1.0,
            max_y: pixmap.height() as f64 - 1.0,
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
            let top = a as u32 * (256 - wx) + b as u32 * wx;
            let bottom = c as u32 * (256 - wx) + d as u32 * wx;
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
            ((quad.iter().map(|&p| f(p) as u32).sum::<u32>() + 2) / 4) as u8
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
    let mix = |a: u8, b: u8| ((a as u32 * (256 - weight) + b as u32 * weight + 128) >> 8) as u8;
    PremultipliedColorU8::from_rgba(
        mix(a.red(), b.red()),
        mix(a.green(), b.green()),
        mix(a.blue(), b.blue()),
        mix(a.alpha(), b.alpha()),
    )
    .unwrap_or(a)
}
