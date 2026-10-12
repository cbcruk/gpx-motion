import type { GpxRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { TileSource } from '../tiles/tiles.types'
import type { Viewport } from '../../viewport/viewport.types'

/** What a {@linkcode FlatView} draws and where its pictures go. */
export interface FlatViewOptions {
  /** The track, as GPX text. */
  gpx: string
  /** Size of the map in device pixels. */
  size: Viewport
  /** The track renderer, which says which tiles the map needs. */
  renderer: Pick<GpxRenderer, 'visibleTiles'>
  /** The style the map starts in: a preset name or a style document. */
  style: string
  /** Takes each picture of the map once drawn. */
  onImage: (image: ImageData) => void
}

/**
 * The demo's 2D basemap, which keeps its labels and label keepout across
 * style switches.
 */
export interface FlatView {
  /** The last picture of the map drawn, or `null` before the first. */
  readonly image: ImageData | null
  /** Loads the map's tiles from `source`, kept for later styles, then draws it. */
  load(source: TileSource): Promise<void>
  /**
   * Switches the map to `style`, with the same labels and keepout, and
   * loads and draws it once a source is known. The old picture stays until
   * the new one is drawn; a failed load is logged.
   */
  setStyle(style: string): Promise<void>
  /** Draws labels with `font`, or none with `null`, now and in later styles. */
  setLabels(font: Uint8Array | null): void
  /** Keeps labels out of these `x, y, width, height` rectangles, in device pixels, now and in later styles. */
  setLabelKeepout(rects: Float32Array): void
  /** Stops the map's worker and drops its pictures; later calls do nothing. */
  dispose(): void
}
