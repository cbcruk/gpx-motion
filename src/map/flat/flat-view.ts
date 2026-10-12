import { toImageData } from '../../viewport/viewport'
import { createFlatMap } from './flat-map'
import type { TileSource } from '../tiles/tiles.types'
import type { FlatView, FlatViewOptions } from './flat-view.types'

/**
 * Creates the demo's 2D basemap, which owns its {@linkcode FlatMap} and
 * switches its style in place, keeping the labels and keepout set so far
 * (and the map's worker).
 *
 * A picture or load of an earlier style is dropped quietly, so only the
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
  const map = createFlatMap(gpx, size, style)
  let source: TileSource | null = null
  /** Bumped by each style switch (and disposing), so pictures and loads of earlier styles are dropped. */
  let generation = 0
  /** Whether the current style's tiles are in, so the map can be drawn. */
  let ready = false
  let image: ImageData | null = null

  const draw = (): void => {
    const drawn = generation
    map
      .render()
      .then((rgba) => {
        if (drawn !== generation) return
        image = toImageData(rgba, size)
        onImage(image)
      })
      .catch((err: unknown) => {
        if (drawn === generation) console.warn('2D map failed', err)
      })
  }

  const loadMap = async (from: TileSource): Promise<void> => {
    const loading = generation
    ready = false
    await map.load(from, renderer.visibleTiles(from.maxzoom, style))
    if (loading !== generation) return
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
      generation += 1
      ready = false
      map.setStyle(next)
      if (!source) return
      await loadMap(source).catch((err: unknown) => console.warn('2D map failed', err))
    },
    setLabels(font): void {
      map.setLabels(font)
      if (ready) draw()
    },
    setLabelKeepout(rects): void {
      map.setLabelKeepout(rects)
      if (ready) draw()
    },
    dispose(): void {
      generation += 1
      ready = false
      source = null
      map.dispose()
    },
  }
}
