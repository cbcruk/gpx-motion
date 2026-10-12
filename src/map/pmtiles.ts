import { PmTiles, StepKind } from '../../crates/wasm/pkg/gpx_motion.js'
import { readPmStep } from './layout'
import type { TileSource } from './tiles.types'

/**
 * Opens a remote PMTiles v3 archive of MVT tiles as a {@linkcode TileSource},
 * reading only the byte ranges each tile needs.
 *
 * The server must answer HTTP range requests (and allow them cross-origin).
 * The archive must not change while it is read: a different `ETag` (when the
 * server exposes it) fails the request. Requires `init()` of the wasm module.
 *
 * @example Load tiles from an archive
 * ```ts
 * import { openPmTiles } from "./pmtiles";
 *
 * const source = await openPmTiles("https://example.com/seoul.pmtiles");
 * const bytes = await source.fetchTile(14, 13970, 6344);
 * ```
 */
export async function openPmTiles(url: string): Promise<TileSource> {
  let etag: string | null = null
  const fetchRange = async (
    offset: number,
    length: number,
    partial = false,
  ): Promise<Uint8Array> => {
    const response = await fetch(url, {
      headers: { Range: `bytes=${offset}-${offset + length - 1}` },
    })
    // A small archive may come whole for the opening read.
    const size = Number(response.headers.get('Content-Length') ?? Infinity)
    const whole = response.status === 200 && partial && offset === 0 && size <= length
    if (response.status !== 206 && !whole) {
      void response.body?.cancel()
      throw new Error(
        response.status === 200
          ? `${url}: the server ignores HTTP range requests`
          : `${url}: HTTP ${response.status}`,
      )
    }
    const tag = response.headers.get('ETag')
    if (etag !== null && tag !== null && tag !== etag) {
      throw new Error(`${url}: the archive changed while it was being read`)
    }
    etag ??= tag
    const bytes = new Uint8Array(await response.arrayBuffer())
    if (bytes.length > length || (!partial && bytes.length < length)) {
      throw new Error(`${url}: the server sent the wrong number of bytes`)
    }
    return bytes
  }

  const prefix = await fetchRange(0, PmTiles.prefixLength(), true)
  const [rootOffset, rootLength] = PmTiles.rootRange(prefix)
  const root =
    rootOffset + rootLength <= prefix.length
      ? prefix.subarray(rootOffset, rootOffset + rootLength)
      : await fetchRange(rootOffset, rootLength)
  const archive = new PmTiles(prefix, root)
  // Tiles load concurrently; each leaf directory is fetched once.
  const leaves = new Map<number, Promise<void>>()
  const loadLeaf = (key: number, offset: number, length: number): Promise<void> => {
    let pending = leaves.get(key)
    if (!pending) {
      pending = fetchRange(offset, length).then((bytes) => archive.addLeaf(key, bytes))
      pending.catch(() => leaves.delete(key))
      leaves.set(key, pending)
    }
    return pending
  }

  return {
    maxzoom: archive.maxZoom,
    async fetchTile(zoom, x, y) {
      for (;;) {
        const step = readPmStep(archive.locate(zoom, x, y))
        if (step.kind === StepKind.Missing) return null
        if (step.kind === StepKind.Tile)
          return archive.tile(await fetchRange(step.offset, step.length))
        await loadLeaf(step.key, step.offset, step.length)
      }
    },
  }
}
