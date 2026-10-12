import type { GlLabels } from '../../../crates/wasm/pkg/gpx_motion.js'

/** Draws a WebGL scene's map labels on a 2D canvas laid over it. */
export interface LabelOverlay {
  /** Replaces the drawn labels with `labels` (cleared when `undefined`); the caller still owns `labels` and frees them after. */
  draw(labels: GlLabels | undefined): void
}
