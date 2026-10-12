import { toImageData } from '../../viewport'
import { createFlatMap } from './flat-map'
import type { FlatMap } from './flat-map.types'
import type { TileSource } from '../tiles.types'
import type { FlatView, FlatViewOptions } from './flat-view.types'

/**
 * Creates the demo's 2D basemap, which owns its {@linkcode FlatMap} and
 * replaces it on a style switch with the labels and keepout set so far.
 *
 * A map replaced while it loads or draws is dropped quietly, so only the
 * current style's picture reaches `onImage`.
 *
 * @example Draw the basemap, then switch its style
 * ```ts
 * import { createFlatView } from "./map/flat/flat-view";
 *
 * const view = createFlatView({ gpx, size, renderer, style: "dark",
 *   onImage: (image) => ctx.putImageData(image, 0, 0) });
 * await view.load(source);
 * await view.setStyle("light");
 * ```
 */
export function createFlatView(options: FlatViewOptions): FlatView {
  const { gpx, size, renderer, onImage } = options
  let style = options.style
  let map: FlatMap = createFlatMap(gpx, size, style)
  let source: TileSource | null = null
  let font: Uint8Array | null = null
  let keepout: Float32Array | null = null
  /** Whether the current map's tiles are in, so it can be drawn. */
  let ready = false
  let image: ImageData | null = null

  const draw = (): void => {
    const drawn = map
    drawn
      .render()
      .then((rgba) => {
        if (drawn !== map) return
        image = toImageData(rgba, size)
        onImage(image)
      })
      .catch((err: unknown) => {
        if (drawn === map) console.warn('2D map failed', err)
      })
  }

  const loadMap = async (from: TileSource): Promise<void> => {
    const loading = map
    ready = false
    await loading.load(from, renderer.visibleTiles(from.maxzoom, style))
    if (loading !== map) return
    ready = true
    draw()
  }

  return {
    get image(): ImageData | null {
      return image
    },
    load(from): Promise<void> {
      source = from
      return loadMap(from)
    },
    async setStyle(next): Promise<void> {
      style = next
      map.dispose()
      map = createFlatMap(gpx, size, style)
      ready = false
      if (font) map.setLabels(font)
      if (keepout) map.setLabelKeepout(keepout.slice())
      if (!source) return
      await loadMap(source).catch((err: unknown) => console.warn('2D map failed', err))
    },
    setLabels(next): void {
      font = next
      map.setLabels(next)
      if (ready) draw()
    },
    setLabelKeepout(rects): void {
      keepout = rects
      map.setLabelKeepout(rects.slice())
      if (ready) draw()
    },
  }
}
