import './style.css'
import { createAppShell } from './app/app-shell'
import { createCameraRig } from './app/camera-rig'
import { createFollowLabels, loadGlMap, tryGlScene } from './app/gl-map'
import { createLabelKeepout } from './app/label-keepout'
import { openMapSource } from './app/map-source'
import { createTerrainToggle, loadMapLabels } from './app/map-toggles'
import { loadTrack } from './app/track'
import { createViewModes } from './app/view-mode'
import * as config from './config/config'
import { createControls } from './controls/controls'
import { createMapStyles } from './map-style/map-style'
import { createFlatView } from './map/flat/flat-view'
import { attachCameraControl } from './map/gl/camera-control'
import { createGroundLoader } from './map/gl/ground-loader'
import { createChaseCamera } from './playback/chase-camera'
import { createPlayback } from './playback/playback'
import { toImageData } from './viewport'
import type { TileSource } from './map/tiles.types'

async function main(root: HTMLElement): Promise<void> {
  const { gpx, flat, flatSize, scene, sceneSize, terrain } = await loadTrack()
  const pmtilesUrl = config.pmtilesUrl()
  const shell = createAppShell(root, {
    width: config.WIDTH,
    flatSize,
    creditOpenFreeMap: !pmtilesUrl,
  })
  const glView = tryGlScene(shell.glCanvas, scene, flatSize.ratio)
  shell.profileCanvas.width = sceneSize.width
  shell.profileCanvas.height = scene.glProfileHeight

  const params = config.initialViewParams()
  const flatView = createFlatView({
    gpx,
    size: flatSize,
    renderer: flat,
    style: config.MAP_STYLE,
    onImage: (image) => views.mode === '2d' && shell.mapCtx.putImageData(image, 0, 0),
  })
  /** The cached tile source both maps draw from, once known. */
  let mapSource: TileSource | null = null
  const followLabels = createFollowLabels(scene, () => (glView ? mapSource : null))
  const chase = createChaseCamera({
    track: scene,
    bearingDeg: config.BEARING_DEG,
    orbitDeg: config.ORBIT_DEG,
    degPerS: config.CHASE_DEG_PER_S,
    headingWindow: config.HEADING_WINDOW,
  })
  const rig = createCameraRig({ scene, params, chase, onFollow: () => followLabels.load() })
  const styles = createMapStyles({
    initial: config.MAP_STYLE,
    documents: { liberty: config.LIBERTY_URL },
    apply: (style) => scene.setMapStyle(style),
    revert: (name) => (params.style = name),
  })
  /** Redraws both maps in the style picked: the scene's colors, the 3D ground tiles, and a new 2D map. */
  const switchStyle = async (): Promise<void> => {
    if (!(await styles.switchTo(params.style))) return
    glView?.setBackground()
    if (glView && mapSource) glView.setGround(createGroundLoader(mapSource, styles.style))
    await flatView.setStyle(styles.style)
  }
  const controls = createControls(
    shell.controls,
    params,
    { follow: rig.applyFollow, reset: rig.reset, style: () => void switchStyle() },
    config.FOLLOW_ZOOM,
  )
  const views = createViewModes(shell, {
    has3d: glView !== null,
    controls,
    tails: config.TAIL,
    flatImage: () => flatView.image,
  })
  attachCameraControl(shell.stage, {
    enabled: () => views.mode === '3d' && glView !== null,
    turn: rig.turn,
    tilt: rig.tilt,
    zoom: rig.zoom,
  })

  const toggles = { shell, scene, glView, flatView, ratio: flatSize.ratio }
  const terrainToggle = createTerrainToggle(toggles, terrain, () => keepLabelsOffAttribution())
  const keepLabelsOffAttribution = createLabelKeepout(
    shell.stage,
    shell.attribution,
    config.WIDTH,
    (rect) => {
      scene.setLabelKeepout(rect)
      flatView.setLabelKeepout(rect.map((v) => v * flatSize.ratio))
    },
  )
  keepLabelsOffAttribution()
  window.addEventListener('resize', keepLabelsOffAttribution)
  const labels = loadMapLabels(toggles)

  void openMapSource(pmtilesUrl)
    .then(async (source) => {
      mapSource = source
      if (params.follow) followLabels.load()
      await Promise.all([
        flatView.load(source),
        glView &&
          loadGlMap(source, {
            scene,
            view: glView,
            style: styles.style,
            terrainUrl: terrain ? config.TERRAIN_URL : null,
            onTerrain: terrainToggle.loaded,
          }),
      ])
    })
    .catch((err: unknown) => console.warn('map tiles failed; drawing without a map', err))

  const playback = createPlayback(config.MAX_FRAME_S)
  const frame = (now: number): void => {
    const in3d = views.mode === '3d' && glView !== null
    const loopMs = views.mode === '3d' ? rig.loopMs : config.LOOP_MS
    const { u, dt, wrapped } = playback.step(now, params.speed, loopMs)
    if (views.mode === '3d') rig.orbit(dt)
    // Starting over, the marker jumps back: so does a chase camera.
    if (wrapped) chase.snap()
    if (in3d) {
      if (params.pitch !== scene.pitchDeg) scene.setPitch(params.pitch)
      glView.setTail(params.tail)
      const chasing = params.follow && params.heading
      const turn = chase.turnAt({
        u,
        dt,
        rotate: params.rotate,
        chasing,
        speed: params.speed,
        loopMs: rig.loopMs,
      })
      glView.render(u, turn)
      labels.overlay?.draw(scene.glLabels(turn, u))
      shell.profileCtx.putImageData(
        toImageData(scene.glProfile(u), { ...sceneSize, height: scene.glProfileHeight }),
        0,
        0,
      )
    } else {
      shell.trackCtx.putImageData(toImageData(flat.renderOverlay(u, params.tail), flatSize), 0, 0)
    }
    requestAnimationFrame(frame)
  }
  requestAnimationFrame(frame)
}

const root = document.querySelector<HTMLElement>('#app')
if (!root) throw new Error('index.html has no #app element')
main(root).catch((err: unknown) => {
  root.textContent = `Error: ${String(err)}`
  console.error(err)
})
