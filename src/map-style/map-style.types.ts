import type { MapStyleName } from '../controls/controls.types'

/** How a {@linkcode MapStyles} resolves and applies basemap styles. */
export interface MapStylesOptions {
  /** The style shown at first, a preset the renderers know by name. */
  initial: MapStyleName
  /** Where each style that is a style document (not a preset) is fetched from. */
  documents: Partial<Record<MapStyleName, string>>
  /** Draws the maps in `style` (a preset name or a style document); throws when it can't read it. */
  apply: (style: string) => void
  /** Puts the style picker back on `name` after a switch to another failed. */
  revert: (name: MapStyleName) => void
}

/** The basemap style the demo shows, switched one request at a time. */
export interface MapStyles {
  /** The style shown now. */
  readonly name: MapStyleName
  /** The style shown now, as the renderers take it: a preset name or a style document. */
  readonly style: string
  /**
   * Resolves style `name` and applies it.
   *
   * @returns `true` once applied; `false` when it failed (and was reverted) or a later switch overtook it.
   */
  switchTo(name: MapStyleName): Promise<boolean>
}
