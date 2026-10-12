/**
 * Fetches the text at `url`, failing on an HTTP error.
 *
 * @example Fetch a style document
 * ```ts
 * import { fetchText } from "./map-style/map-style.utils";
 *
 * const style = await fetchText("https://tiles.openfreemap.org/styles/liberty");
 * ```
 */
export async function fetchText(url: string): Promise<string> {
  const response = await fetch(url)
  if (!response.ok) throw new Error(`${url}: HTTP ${response.status}`)
  return response.text()
}
