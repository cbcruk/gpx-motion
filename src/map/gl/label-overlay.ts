import type { LabelOverlay } from "./label-overlay.types";

/** Floats per label in `GlLabels.labels()`. */
const LABEL_STRIDE = 8;
const FONT_FAMILY = "gpx-motion-labels";

const css = ([r, g, b, a]: Float32Array, opacity: number): string =>
  `rgba(${r * 255}, ${g * 255}, ${b * 255}, ${a * opacity})`;

/**
 * Draws the labels a scene places for its WebGL view (`SceneRenderer.glLabels`)
 * with the same font, so they match the placement's measurements: point
 * labels on their baseline, line labels one turned character at a time, each
 * over a halo.
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
      ctx.lineJoin = "round";
      texts.forEach((text, i) => {
        const [x, y, size, opacity, haloWidth, rise, first, count] = values.subarray(
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
        const chars = Array.from(text).slice(0, count);
        // All halos first, so one character's halo never covers its neighbour.
        for (const paint of ["stroke", "fill"] as const) {
          chars.forEach((ch, k) => {
            const [gx, gy, angle] = glyphs.subarray((first + k) * 3, (first + k) * 3 + 3);
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
