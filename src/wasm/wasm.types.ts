/** A wasm-bindgen object (such as `Tile` or `GlLabels`) whose memory must be freed explicitly. */
export interface Freeable {
  /** Frees the object's wasm memory; it must not be used afterwards. */
  free(): void
}
