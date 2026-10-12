import type { GpxRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import { createLifetime } from '../../lifetime/lifetime'
import { loadVectorTiles } from '../tiles/tiles'
import type { TileSink } from '../tiles/tiles.types'
import type { Viewport } from '../../viewport/viewport.types'
import { createWorkerClient } from '../../worker/worker-client'
import { applyChange, createFlatRenderer, tilePlace } from './flat-map.utils'
import type { FlatApi, FlatChange, FlatMap } from './flat-map.types'

/**
 * Creates the 2D basemap of the `gpx` track at `size`, in `style`, decoded
 * and drawn in a Web Worker so loading never stalls the page. If the worker
 * fails, the map is drawn on the main thread instead, from the same tiles
 * and settings. Switching style keeps the worker and its wasm core.
 *
 * @example Load and draw the basemap
 * ```ts
 * import type { GpxRenderer } from '@wasm'
 * import { createFlatMap } from '@/map/flat/flat-map'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 * import { viewport } from '@/viewport/viewport'
 *
 * declare const gpx: string
 * declare const renderer: GpxRenderer
 * declare const source: TileSource
 *
 * const map = createFlatMap(gpx, viewport(640, 640, 2), 'dark')
 * await map.load(source, renderer.visibleTiles(source.maxzoom, 'dark'))
 * const rgba = await map.render()
 * ```
 */
export function createFlatMap(gpx: string, size: Viewport, style: string): FlatMap {
  const setup = { gpx, size: { width: size.width, height: size.height, ratio: size.ratio } }
  /** This style's tiles given to the worker, to replay on the main thread if it fails. */
  const tiles: FlatChange[] = []
  /** The latest labels and keepout, replayed into each new renderer (fallback or style switch). */
  const latest = new Map<FlatChange['type'], FlatChange>()
  let local: GpxRenderer | null = null
  let running: Promise<Uint8Array> | null = null
  let queued: Promise<Uint8Array> | null = null
  /** Bumped by each style switch, so tiles of loads started before it are dropped. */
  let epoch = 0
  const life = createLifetime()
  life.defer(() => {
    local?.free()
    local = null
    tiles.length = 0
    latest.clear()
  })

  /** Applies `changes` to the main-thread renderer, logging (not throwing) those that fail. */
  const replay = (renderer: GpxRenderer, changes: FlatChange[]): void => {
    for (const change of changes) {
      try {
        applyChange(renderer, change)
      } catch (err) {
        console.warn(`2D map ${change.type} failed`, err)
      }
    }
  }
  const drawLocally = (reason: Error): void => {
    console.warn('2D map worker failed; drawing the map on the main thread', reason)
    local = createFlatRenderer(setup)
    replay(local, [...tiles, ...latest.values()])
    tiles.length = 0
  }
  const worker = createWorkerClient<FlatApi>({
    name: '2D map worker',
    create: () => new Worker(new URL('./flat-worker.ts', import.meta.url), { type: 'module' }),
    onFailure: drawLocally,
  })
  life.defer(() => worker.dispose())
  worker.post('init', { ...setup, style })

  const apply = (change: FlatChange): void => {
    if (life.ended) return
    if (change.type !== 'tile') latest.set(change.type, change)
    if (local) {
      applyChange(local, change)
      return
    }
    if (change.type === 'tile') {
      tiles.push(change)
      const bytes = change.bytes.slice()
      worker.post('change', { ...change, bytes }, [bytes.buffer])
    } else {
      worker.post('change', change)
    }
  }

  const draw = async (): Promise<Uint8Array> => {
    if (life.ended) throw new Error('2D map disposed')
    if (!local) {
      try {
        return await worker.call('render', undefined)
      } catch (err) {
        if (!local) throw err
      }
    }
    if (life.ended || !local) throw new Error('2D map disposed')
    return local.renderMap(style)
  }

  const render = (): Promise<Uint8Array> => {
    if (!running) {
      running = draw().finally(() => {
        running = null
      })
      return running
    }
    queued ??= running
      .catch(() => undefined)
      .then(() => {
        queued = null
        return render()
      })
    return queued
  }

  return {
    async load(source, tiles): Promise<void> {
      const at = epoch
      const sink: TileSink = {
        addVectorTile: (tile, bytes) => {
          if (at === epoch) apply({ type: 'tile', tile: tilePlace(tile), bytes })
        },
      }
      await loadVectorTiles(sink, source, tiles)
    },
    setStyle(next): void {
      if (life.ended) return
      style = next
      epoch += 1
      tiles.length = 0
      if (local) {
        local.free()
        local = null
        local = createFlatRenderer(setup)
        replay(local, [...latest.values()])
        return
      }
      worker.post('style', next)
      for (const change of latest.values()) worker.post('change', change)
    },
    setLabels(font): void {
      apply({ type: 'labels', font: font && font.slice() })
    },
    setLabelKeepout(rects): void {
      apply({ type: 'keepout', rects: rects.slice() })
    },
    render,
    dispose(): void {
      life.end()
    },
  }
}
