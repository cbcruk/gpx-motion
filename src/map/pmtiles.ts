import { PmTiles } from "../../crates/wasm/pkg/gpx_motion.js";
import type { TileSource } from "./tiles.types";

/** What {@linkcode PmTiles.locate} answers, by its first number. */
const MISSING = 0;
const TILE = 1;

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
  let etag: string | null = null;
  const fetchRange = async (
    offset: number,
    length: number,
    partial = false,
  ): Promise<Uint8Array> => {
    const response = await fetch(url, {
      headers: { Range: `bytes=${offset}-${offset + length - 1}` },
    });
    if (response.status === 200) {
      throw new Error(`${url}: the server ignores HTTP range requests`);
    }
    if (response.status !== 206) throw new Error(`${url}: HTTP ${response.status}`);
    const tag = response.headers.get("ETag");
    if (etag !== null && tag !== null && tag !== etag) {
      throw new Error(`${url}: the archive changed while it was being read`);
    }
    etag ??= tag;
    const bytes = new Uint8Array(await response.arrayBuffer());
    if (bytes.length > length || (!partial && bytes.length < length)) {
      throw new Error(`${url}: the server sent the wrong number of bytes`);
    }
    return bytes;
  };

  const prefix = await fetchRange(0, PmTiles.prefixLength(), true);
  const [rootOffset, rootLength] = PmTiles.rootRange(prefix);
  const root =
    rootOffset + rootLength <= prefix.length
      ? prefix.subarray(rootOffset, rootOffset + rootLength)
      : await fetchRange(rootOffset, rootLength);
  const archive = new PmTiles(prefix, root);

  return {
    maxzoom: archive.maxZoom,
    async fetchTile(zoom, x, y) {
      for (;;) {
        const [kind, offset, length, key] = archive.locate(zoom, x, y);
        if (kind === MISSING) return null;
        const bytes = await fetchRange(offset, length);
        if (kind === TILE) return archive.tile(bytes);
        archive.addLeaf(key, bytes);
      }
    },
  };
}
