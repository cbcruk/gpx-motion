import * as config from '../../config/config'
import { createLabelOverlay } from '../../map/gl/label-overlay/label-overlay'
import { loadLabelFont } from '../../map/labels/labels'
import type { LabelOverlay } from '../../map/gl/label-overlay/label-overlay.types'
import type { MapLabels, MapTogglesOptions, TerrainToggle } from './map-toggles.types'

/**
 * Wires the Terrain checkbox of `shell`: hidden without terrain or WebGL,
 * enabled with its credit once terrain loads, unchecked if none did.
 *
 * @returns What hears whether terrain loaded (see `loadGlMap`'s `onTerrain`).
 *
 * @example Wire the checkbox
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import type { AppShell } from '@/app/app-shell/app-shell.types'
 * import { createTerrainToggle } from '@/app/map-toggles/map-toggles'
 * import type { FlatView } from '@/map/flat/flat-view.types'
 * import type { GlScene } from '@/map/gl/gl-scene/gl-scene.types'
 *
 * declare const shell: AppShell
 * declare const scene: SceneRenderer
 * declare const glView: GlScene | null
 * declare const flatView: FlatView
 *
 * const toggle = createTerrainToggle({ shell, scene, glView, flatView, ratio: 2 }, true, () => {})
 * toggle.loaded(true)
 * ```
 */
export function createTerrainToggle(
  options: MapTogglesOptions,
  available: boolean,
  onShown: () => void,
): TerrainToggle {
  const { shell, scene, glView } = options
  const input = shell.terrainInput
  shell.terrainToggle.hidden = !available || !glView
  input.addEventListener('change', () => {
    scene.setTerrainVisible(input.checked)
    glView?.setTerrainVisible(input.checked)
  })
  return {
    loaded: (loaded) => {
      if (!loaded) {
        input.checked = false
        return false
      }
      input.disabled = false
      shell.terrainCredit.hidden = false
      onShown()
      return true
    },
  }
}

/**
 * Fetches the label font and wires the Labels checkbox of `shell` to draw
 * labels on both maps; the 3D labels are drawn on the track canvas.
 *
 * Without the font, the checkbox stays off and disabled and its credit is
 * hidden.
 *
 * @example Draw the 3D labels each frame
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import type { AppShell } from '@/app/app-shell/app-shell.types'
 * import { loadMapLabels } from '@/app/map-toggles/map-toggles'
 * import type { FlatView } from '@/map/flat/flat-view.types'
 * import type { GlScene } from '@/map/gl/gl-scene/gl-scene.types'
 * import { withFree } from '@/wasm/wasm.utils'
 *
 * declare const shell: AppShell
 * declare const scene: SceneRenderer
 * declare const glView: GlScene | null
 * declare const flatView: FlatView
 * declare const turn: number
 * declare const u: number
 *
 * const labels = loadMapLabels({ shell, scene, glView, flatView, ratio: 2 })
 * const overlay = labels.overlay
 * if (overlay) withFree(scene.glLabels(turn, u), (placed) => overlay.draw(placed))
 * ```
 */
export function loadMapLabels(options: MapTogglesOptions): MapLabels {
  const { shell, scene, glView, flatView, ratio } = options
  const input = shell.labelsInput
  let overlay: LabelOverlay | null = null
  void loadLabelFont(config.LABEL_FONT_URL).then((font) => {
    input.disabled = !font
    if (!font) {
      input.checked = false
      shell.fontCredit.hidden = true
      return
    }
    if (glView) {
      createLabelOverlay(shell.trackCanvas, ratio, font)
        .then((created) => (overlay = created))
        .catch((err: unknown) => console.warn('label font failed; drawing 3D without labels', err))
    }
    const apply = (): void => {
      flatView.setLabels(input.checked ? font : null)
      if (input.checked) scene.setLabels(font)
      else scene.clearLabels()
    }
    input.addEventListener('change', apply)
    apply()
  })
  return {
    get overlay(): LabelOverlay | null {
      return overlay
    },
  }
}
