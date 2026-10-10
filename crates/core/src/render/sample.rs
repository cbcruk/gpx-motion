use tiny_skia::{Pixmap, PremultipliedColorU8};

/// Borrowed pixels of a source view for fast bilinear lookups.
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

    /// Bilinear sample at pixel coordinates (pixel centers on integers), or
    /// `None` outside the image. Points within the outer half pixel clamp to
    /// the edge pixels.
    pub(crate) fn sample(&self, x: f64, y: f64) -> Option<PremultipliedColorU8> {
        if !(x >= -0.5 && y >= -0.5 && x <= self.max_x + 0.5 && y <= self.max_y + 0.5) {
            return None;
        }
        // Snap to 1/256 pixel so exact pixel centers sample exactly.
        let fx = (x.clamp(0.0, self.max_x) * 256.0 + 0.5) as usize;
        let fy = (y.clamp(0.0, self.max_y) * 256.0 + 0.5) as usize;
        Some(self.sample_fixed(fx, fy))
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
