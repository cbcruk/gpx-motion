import init from '../../crates/wasm/pkg/gpx_motion.js'

/** Where the built wasm core (`vp run build:wasm`) is served from, bundled as an asset. */
export const WASM_URL = new URL('../../crates/wasm/pkg/gpx_motion_bg.wasm', import.meta.url)

let started: Promise<void> | null = null

/**
 * Fetches and starts the wasm core in this thread (the page or a worker),
 * once: later calls share the first.
 *
 * @example Start the core before using it
 * ```ts
 * import { GpxRenderer } from '@wasm'
 * import { initWasm } from '@/wasm/wasm'
 *
 * declare const gpx: string
 *
 * await initWasm()
 * const renderer = new GpxRenderer(gpx, 640, 640, 1)
 * ```
 */
export function initWasm(): Promise<void> {
  started ??= init({ module_or_path: WASM_URL }).then(() => undefined)
  return started
}
