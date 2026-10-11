import type { LabelOverlay } from "./label-overlay.types";

/** Floats per label in `GlLabels.labels()`. */
const LABEL_STRIDE = 9;
/** Floats per glyph in `GlLabels.glyphs()`. */
const GLYPH_STRIDE = 4;
const FONT_FAMILY = "gpx-motion-labels";

const css = ([r, g, b, a]: Float32Array, opacity: number): string =>
  `rgba(${r * 255}, ${g * 255}, ${b * 255}, ${a * opacity})`;

/**
 * Draws the labels a scene places for its WebGL view (`SceneRenderer.glLabels`)
 * with the same font, so they match the placement's measurements: point
 * labels on their baseline, line labels one turned character at a time
 * (leaving out those the placement marks covered), each over a halo. Line
 * labels in shaped scripts (Arabic, Hebrew, Indic, …) are drawn whole along
 * the line instead, so the browser can join and order their letters.
 *
 * `canvas` covers the scene, `ratio` device pixels per scene pixel; `font`
 * holds the label font's bytes.
 *
 * @example Draw labels every frame
 * ```ts
 * import { createLabelOverlay } from "./gl/label-overlay";
 *
 * const overlay = await createLabelOverlay(canvas, 2, fontBytes);
 * overlay.draw(scene.glLabels(turn));
 * ```
 */
export async function createLabelOverlay(
  canvas: HTMLCanvasElement,
  ratio: number,
  font: Uint8Array,
): Promise<LabelOverlay> {
  const face = new FontFace(FONT_FAMILY, font.slice().buffer);
  document.fonts.add(await face.load());
  const ctx = canvas.getContext("2d")!;
  return {
    draw(labels) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      if (!labels) return;
      const texts = labels.texts();
      const values = labels.labels();
      const glyphs = labels.glyphs();
      const [color, halo] = [labels.color, labels.halo];
      labels.free();
      ctx.setTransform(ratio, 0, 0, ratio, 0, 0);
      ctx.textBaseline = "alphabetic";
      // The core measures plain advances, without kerning.
      ctx.fontKerning = "none";
      ctx.lineJoin = "round";
      texts.forEach((text, i) => {
        const [x, y, size, opacity, haloWidth, rise, first, count, whole] = values.subarray(
          i * LABEL_STRIDE,
          (i + 1) * LABEL_STRIDE,
        );
        ctx.font = `${size}px "${FONT_FAMILY}"`;
        ctx.lineWidth = haloWidth * 2;
        ctx.strokeStyle = css(halo, opacity);
        ctx.fillStyle = css(color, opacity);
        if (count === 0) {
          ctx.strokeText(text, x, y);
          ctx.fillText(text, x, y);
          return;
        }
        if (whole === 1) {
          // The browser shapes the text itself, kerning included, so it is
          // drawn whole: centered between the first and last glyph, along
          // the line between them (or the one glyph's own angle).
          const at = (k: number): Float32Array =>
            glyphs.subarray((first + k) * GLYPH_STRIDE, (first + k + 1) * GLYPH_STRIDE);
          const [x0, y0, a0] = at(0);
          const [x1, y1] = at(count - 1);
          ctx.save();
          ctx.translate((x0 + x1) / 2, (y0 + y1) / 2);
          ctx.rotate(count > 1 ? Math.atan2(y1 - y0, x1 - x0) : a0);
          ctx.textAlign = "center";
          ctx.fontKerning = "normal";
          ctx.strokeText(text, 0, rise);
          ctx.fillText(text, 0, rise);
          ctx.restore();
          return;
        }
        const chars = Array.from(text).slice(0, count);
        // All halos first, so one character's halo never covers its neighbour.
        for (const paint of ["stroke", "fill"] as const) {
          chars.forEach((ch, k) => {
            const [gx, gy, angle, shown] = glyphs.subarray(
              (first + k) * GLYPH_STRIDE,
              (first + k + 1) * GLYPH_STRIDE,
            );
            // Where labels keep clear of (the track, a panel): left out.
            if (shown === 0) return;
            ctx.save();
            ctx.translate(gx, gy);
            ctx.rotate(angle);
            const half = ctx.measureText(ch).width / 2;
            if (paint === "stroke") ctx.strokeText(ch, -half, rise);
            else ctx.fillText(ch, -half, rise);
            ctx.restore();
          });
        }
      });
    },
  };
}
