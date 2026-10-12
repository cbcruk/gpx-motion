/// <reference types="node" />
import { readFileSync } from 'node:fs'
import { initSync } from '../../crates/wasm/pkg/gpx_motion.js'

/** Loads the built wasm module (`vp run build:wasm`) for tests in Node, where `init()` cannot fetch it. */
export function loadWasm(): void {
  const url = new URL('../../crates/wasm/pkg/gpx_motion_bg.wasm', import.meta.url)
  initSync({ module: readFileSync(url) })
}

/** Reads a file under `samples/` as text, such as the public `namsan.gpx`. */
export function readSample(name: string): string {
  return readFileSync(new URL(`../../samples/${name}`, import.meta.url), 'utf8')
}
