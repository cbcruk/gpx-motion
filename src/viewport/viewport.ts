import type { Viewport } from './viewport.types'

/**
 * Sizes a canvas of `width`×`height` CSS pixels in device pixels.
 *
 * @example Size a canvas for the current screen
 * ```ts
 * import { viewport } from '@/viewport/viewport'
 *
 * const size = viewport(640, 640, window.devicePixelRatio)
 * ```
 */
export function viewport(width: number, height: number, ratio: number): Viewport {
  return { width: Math.round(width * ratio), height: Math.round(height * ratio), ratio }
}

/**
 * Wraps straight RGBA bytes drawn at `size` as an `ImageData`, copying them.
 *
 * @example Show a rendered map on a canvas
 * ```ts
 * import type { GpxRenderer } from '@wasm'
 * import { toImageData, viewport } from '@/viewport/viewport'
 *
 * declare const renderer: GpxRenderer
 * declare const ctx: CanvasRenderingContext2D
 *
 * const size = viewport(640, 640, 2)
 * ctx.putImageData(toImageData(renderer.renderMap('dark'), size), 0, 0)
 * ```
 */
export function toImageData(rgba: Uint8Array, size: Viewport): ImageData {
  return new ImageData(new Uint8ClampedArray(rgba), size.width, size.height)
}
