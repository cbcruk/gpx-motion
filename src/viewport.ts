import type { Viewport } from './viewport.types'

/**
 * Sizes a canvas of `width`×`height` CSS pixels in device pixels.
 *
 * @example Size a canvas for the current screen
 * ```ts
 * import { viewport } from "./viewport";
 *
 * const size = viewport(640, 640, window.devicePixelRatio);
 * ```
 */
export function viewport(width: number, height: number, ratio: number): Viewport {
  return { width: Math.round(width * ratio), height: Math.round(height * ratio), ratio }
}
