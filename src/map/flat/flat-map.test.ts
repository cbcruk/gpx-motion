import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test'
import { GpxRenderer } from '../../../crates/wasm/pkg/gpx_motion.js'
import type { TileSource } from '../tiles/tiles.types'
import { loadWasm, readSample } from '../../wasm/wasm.test-utils'
import { createFlatMap } from './flat-map'

beforeAll(() => {
  loadWasm()
})

afterEach(() => {
  vi.restoreAllMocks()
})

const size = { width: 64, height: 48, ratio: 1 }
const source: TileSource = { maxzoom: 14, fetchTile: () => Promise.resolve(new Uint8Array([1])) }

describe('createFlatMap on the main thread', () => {
  // Node has no `Worker`, so the map falls back to the main thread at once.
  it('keeps labels and keepout across a style switch, and takes tiles again', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    const setLabels = vi.spyOn(GpxRenderer.prototype, 'setLabels').mockImplementation(() => {})
    const setKeepout = vi.spyOn(GpxRenderer.prototype, 'setLabelKeepout')
    const addTile = vi.spyOn(GpxRenderer.prototype, 'addVectorTile').mockImplementation(() => {})
    const gpx = readSample('namsan.gpx')
    const tiles = (): ReturnType<GpxRenderer['visibleTiles']> =>
      new GpxRenderer(gpx, size.width, size.height, 1).visibleTiles(14, 'dark')
    const font = new Uint8Array([1, 2, 3])
    const rects = new Float32Array([0, 0, 10, 10])

    const map = createFlatMap(gpx, size, 'dark')
    map.setLabels(font)
    map.setLabelKeepout(rects)
    await map.load(source, tiles())
    const before = addTile.mock.calls.length
    expect(before).toBeGreaterThan(0)

    map.setStyle('light')
    expect(setLabels).toHaveBeenCalledTimes(2)
    expect(setLabels.mock.calls[1]?.[0]).toEqual(font)
    expect(setKeepout).toHaveBeenCalledTimes(2)
    expect(setKeepout.mock.calls[1]?.[0]).toEqual(rects)
    expect(setLabels.mock.contexts[1]).not.toBe(setLabels.mock.contexts[0])

    await map.load(source, tiles())
    expect(addTile.mock.calls.length).toBe(before * 2)
    expect(addTile.mock.contexts.at(-1)).toBe(setLabels.mock.contexts[1])
    expect(await map.render()).toHaveLength(size.width * size.height * 4)
    map.dispose()
  })
})
