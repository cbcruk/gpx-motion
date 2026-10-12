import type { TileSource } from '../tiles.types'
import type { Tile } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { Viewport } from '../../viewport.types'

/** A map tile's id and placement in viewport pixels, as plain data a worker can receive. */
export interface TilePlace {
  /** Tile zoom. */
  zoom: number
  /** Tile column. */
  x: number
  /** Tile row. */
  y: number
  /** Left edge in viewport pixels. */
  dx: number
  /** Top edge in viewport pixels. */
  dy: number
  /** Edge length in viewport pixels: 256, or larger when overzoomed. */
  size: number
}

/** A change to what the 2D map draws. */
export type FlatChange =
  | { type: 'tile'; tile: TilePlace; bytes: Uint8Array }
  | { type: 'labels'; font: Uint8Array | null }
  | { type: 'keepout'; rects: Float32Array }

/** The track and canvas a 2D map renderer draws, as plain data a worker can receive. */
export interface FlatSetup {
  /** The track, as GPX text. */
  gpx: string
  /** Size of the map in device pixels. */
  size: Viewport
}

/** The messages the 2D map worker takes (see `serve`), each run after those before it. */
export type FlatApi = {
  /** Starts the wasm core and a renderer of `setup` drawing in `style`. */
  init: (arg: FlatSetup & { style: string }) => void
  /** Starts over in `style` with a fresh renderer: no tiles, labels or keepout. */
  style: (style: string) => void
  /** Applies a change to what the map draws. */
  change: (change: FlatChange) => void
  /** Draws the map with everything applied so far, as straight RGBA bytes. */
  render: (arg: undefined) => Uint8Array
}

/** The 2D basemap of a track, decoded and drawn off the main thread where it can. */
export interface FlatMap {
  /** Fetches `tiles` (from the track renderer's `visibleTiles`) from `source` and adds them, freeing each `Tile`. */
  load(source: TileSource, tiles: Tile[]): Promise<void>
  /** Draws map labels with this font from the next render on, or none with `null`. */
  setLabels(font: Uint8Array | null): void
  /** Keeps labels out of these `x, y, width, height` rectangles, in device pixels. */
  setLabelKeepout(rects: Float32Array): void
  /**
   * Switches the map to `style`, keeping its labels and keepout but dropping
   * its tiles, which the style may pick differently: `load` them again.
   * Tiles of loads started before the switch are ignored.
   */
  setStyle(style: string): void
  /**
   * Draws the map with everything set so far, as straight RGBA bytes.
   *
   * While a render runs, later calls share the one after it, so a burst of
   * changes draws at most twice.
   */
  render(): Promise<Uint8Array>
  /** Stops the worker and drops what was loaded; later calls do nothing (renders reject). */
  dispose(): void
}
