/// <reference types="node" />
import { readFileSync } from 'node:fs'
import { initSync } from '../../crates/wasm/pkg/gpx_motion.js'
import { WASM_URL } from './wasm'

/** Loads the built wasm module (`vp run build:wasm`) for tests in Node, where `init()` cannot fetch it. */
export function loadWasm(): void {
  initSync({ module: readFileSync(WASM_URL) })
}

/** Reads a file under `samples/` as text, such as the public `namsan.gpx`. */
export function readSample(name: string): string {
  return readFileSync(new URL(`../../samples/${name}`, import.meta.url), 'utf8')
}
