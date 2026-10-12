import { fetchOk } from '../../fetch/fetch.utils'

/**
 * Fetches the font map labels are drawn with, as TrueType/OpenType bytes.
 *
 * Labels need a font with glyphs for the map's script (Korean for the
 * demo's tracks). A failed fetch rejects; the caller decides how to draw
 * without labels.
 *
 * @example Load a label font
 * ```ts
 * import type { GpxRenderer } from '@wasm'
 * import { loadLabelFont } from '@/map/labels/labels'
 *
 * declare const renderer: GpxRenderer
 *
 * const font = await loadLabelFont('https://example.com/NanumGothic-Regular.ttf')
 * renderer.setLabels(font)
 * ```
 */
export async function loadLabelFont(url: string, signal?: AbortSignal): Promise<Uint8Array> {
  return new Uint8Array(await (await fetchOk(url, signal)).arrayBuffer())
}
