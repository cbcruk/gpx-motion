import { fetchOk } from '../fetch/fetch.utils'

/**
 * Fetches the font map labels are drawn with, as TrueType/OpenType bytes.
 *
 * Labels need a font with glyphs for the map's script (Korean for the
 * demo's tracks). A failed fetch is logged and yields `null`, so the map
 * still renders without labels.
 *
 * @example Load a label font
 * ```ts
 * import { loadLabelFont } from "./labels";
 *
 * const font = await loadLabelFont("https://example.com/NanumGothic-Regular.ttf");
 * if (font) renderer.setLabels(font);
 * ```
 */
export async function loadLabelFont(url: string): Promise<Uint8Array | null> {
  try {
    const response = await fetchOk(url)
    return new Uint8Array(await response.arrayBuffer())
  } catch (err) {
    console.warn(`label font failed: ${url}`, err)
    return null
  }
}
