import type { MapStyleName } from '../controls/controls.types'
import { fetchText } from './map-style.utils'
import type { MapStyles, MapStylesOptions } from './map-style.types'

/**
 * Creates the demo's basemap style switcher.
 *
 * A preset is applied by name; a style document is fetched the first time
 * and kept once it applies. When switches overlap, only the latest is
 * applied. A style that fails to fetch or apply leaves the current one
 * shown and puts the picker back on it.
 *
 * @example Switch to the liberty style
 * ```ts
 * import { createMapStyles } from "./map-style/map-style";
 *
 * const styles = createMapStyles({
 *   initial: "dark",
 *   documents: { liberty: "https://tiles.openfreemap.org/styles/liberty" },
 *   apply: (style) => scene.setMapStyle(style),
 *   revert: (name) => (params.style = name),
 * });
 * if (await styles.switchTo("liberty")) redraw(styles.style);
 * ```
 */
export function createMapStyles(options: MapStylesOptions): MapStyles {
  const { documents, apply, revert } = options
  let name: MapStyleName = options.initial
  let style: string = options.initial
  const fetched = new Map<MapStyleName, string>()
  let request = 0

  const resolve = async (next: MapStyleName): Promise<string> => {
    const url = documents[next]
    if (url === undefined) return next
    return fetched.get(next) ?? (await fetchText(url))
  }

  return {
    get name(): MapStyleName {
      return name
    },
    get style(): string {
      return style
    },
    async switchTo(next): Promise<boolean> {
      const id = ++request
      try {
        const resolved = await resolve(next)
        if (id !== request) return false
        apply(resolved)
        if (resolved !== next) fetched.set(next, resolved)
        style = resolved
      } catch (err) {
        console.warn(`map style ${next} failed; keeping the current one`, err)
        if (id === request) revert(name)
        return false
      }
      name = next
      return true
    },
  }
}
