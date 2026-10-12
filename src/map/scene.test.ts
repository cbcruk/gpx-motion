import { afterEach, beforeAll, describe, expect, it, vi } from 'vite-plus/test'
import { createSceneRenderer } from './scene'
import type { SceneOptions } from './scene.types'
import { loadWasm, readSample } from './wasm.test-utils'

beforeAll(() => {
  loadWasm()
})

afterEach(() => {
  vi.restoreAllMocks()
})

const options = (size: number, terrain: boolean): SceneOptions => ({
  size: { width: size, height: size, ratio: 1 },
  pitchDeg: 55,
  bearingDeg: 20,
  orbitDeg: 360,
  terrain,
})

/** A straight route about 40 km north, too large for terrain framed in a 2048 px viewport. */
const longRoute = (): string => {
  const points = Array.from({ length: 200 }, (_, i) => {
    const lat = 35.8 + i * 0.0018
    const lon = 129.2 + Math.sin(i * 0.3) * 0.01
    return `<trkpt lat="${lat}" lon="${lon}"><ele>${20 + (i % 40)}</ele></trkpt>`
  })
  return `<gpx version="1.1"><trk><trkseg>${points.join('')}</trkseg></trk></gpx>`
}

describe('createSceneRenderer', () => {
  it('builds over terrain where the route allows it', () => {
    const { scene, terrain } = createSceneRenderer(readSample('namsan.gpx'), options(640, true))
    expect(terrain).toBe(true)
    expect(scene.terrainTiles().length).toBeGreaterThan(0)
  })

  it('builds flat ground when terrain is not asked for', () => {
    const { scene, terrain } = createSceneRenderer(readSample('namsan.gpx'), options(640, false))
    expect(terrain).toBe(false)
    expect(scene.terrainTiles()).toHaveLength(0)
  })

  it('falls back to flat ground and logs why terrain is unavailable', () => {
    const info = vi.spyOn(console, 'info').mockImplementation(() => {})
    const { scene, terrain } = createSceneRenderer(longRoute(), options(2048, true))
    expect(terrain).toBe(false)
    expect(scene.terrainTiles()).toHaveLength(0)
    expect(info).toHaveBeenCalledWith(expect.stringContaining('too large'))
  })

  it('throws what flat ground does not avoid', () => {
    vi.spyOn(console, 'info').mockImplementation(() => {})
    expect(() => createSceneRenderer('<gpx></gpx>', options(640, true))).toThrow()
  })
})
