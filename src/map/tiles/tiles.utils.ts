/**
 * The `zoom/x/y` key of a map tile, for maps, sets, and log messages.
 *
 * @example Key a tile
 * ```ts
 * import { tileKey } from '@/map/tiles/tiles.utils'
 *
 * tileKey(14, 13970, 6344) // '14/13970/6344'
 * ```
 */
export function tileKey(zoom: number, x: number, y: number): string {
  return `${zoom}/${x}/${y}`
}
