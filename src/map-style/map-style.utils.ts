import { fetchOk } from '../fetch/fetch.utils'

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
  return (await fetchOk(url)).text()
}
