import { afterEach, describe, expect, it, vi } from 'vite-plus/test'
import { cachedSource, tileJsonSource } from './tiles'
import type { TileSource } from './tiles.types'

describe('cachedSource', () => {
  const counting = (fetchTile: TileSource['fetchTile']): TileSource & { calls: string[] } => {
    const calls: string[] = []
    return {
      maxzoom: 14,
      calls,
      fetchTile(zoom, x, y) {
        calls.push(`${zoom}/${x}/${y}`)
        return fetchTile(zoom, x, y)
      },
    }
  }

  it('fetches each tile once', async () => {
    const source = counting(() => Promise.resolve(new Uint8Array([1])))
    const cached = cachedSource(source)
    const [a, b] = await Promise.all([cached.fetchTile(1, 2, 3), cached.fetchTile(1, 2, 3)])
    await cached.fetchTile(1, 2, 4)
    expect(a).toBe(b)
    expect(source.calls).toEqual(['1/2/3', '1/2/4'])
    expect(cached.maxzoom).toBe(14)
  })

  it('fetches a failed tile again', async () => {
    let fail = true
    const source = counting(() =>
      fail ? Promise.reject(new Error('offline')) : Promise.resolve(null),
    )
    const cached = cachedSource(source)
    await expect(cached.fetchTile(1, 2, 3)).rejects.toThrow('offline')
    fail = false
    await expect(cached.fetchTile(1, 2, 3)).resolves.toBeNull()
    expect(source.calls).toHaveLength(2)
  })
})

describe('tileJsonSource', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  const stubFetch = (status: number): ReturnType<typeof vi.fn> => {
    const fetch = vi.fn((_url: string) =>
      Promise.resolve(new Response(status === 204 ? null : new Uint8Array([7, 8]), { status })),
    )
    vi.stubGlobal('fetch', fetch)
    return fetch
  }

  it('fills {z}, {x} and {y} into the template', async () => {
    const fetch = stubFetch(200)
    const source = tileJsonSource({ tiles: ['https://t.test/{z}/{x}/{y}.pbf'], maxzoom: 12 })
    expect(await source.fetchTile(5, 6, 7)).toEqual(new Uint8Array([7, 8]))
    expect(fetch).toHaveBeenCalledWith('https://t.test/5/6/7.pbf')
    expect(source.maxzoom).toBe(12)
  })

  it('accepts {zoom} for the zoom', async () => {
    const fetch = stubFetch(200)
    await tileJsonSource({ tiles: ['https://t.test/{zoom}/{x}/{y}'] }).fetchTile(1, 2, 3)
    expect(fetch).toHaveBeenCalledWith('https://t.test/1/2/3')
  })

  it('treats 404 and 204 as no tile', async () => {
    const source = tileJsonSource({ tiles: ['https://t.test/{z}/{x}/{y}'] })
    stubFetch(404)
    expect(await source.fetchTile(1, 2, 3)).toBeNull()
    stubFetch(204)
    expect(await source.fetchTile(1, 2, 3)).toBeNull()
  })

  it('throws on other errors', async () => {
    stubFetch(500)
    const source = tileJsonSource({ tiles: ['https://t.test/{z}/{x}/{y}'] })
    await expect(source.fetchTile(1, 2, 3)).rejects.toThrow('HTTP 500: https://t.test/1/2/3')
  })
})
