import { GpxRenderer } from '../../crates/wasm/pkg/gpx_motion.js'
import * as config from '../config/config'
import { createSceneRenderer } from '../map/scene'
import { gpxUrl } from '../sample/sample'
import { fetchOk } from '../fetch/fetch.utils'
import { viewport } from '../viewport'
import { initWasm } from '../wasm/wasm'
import type { Track } from './track.types'

/**
 * Starts the wasm core, fetches the demo's track (see `gpxUrl`), and builds
 * its 2D renderer at the screen's pixel ratio and its 3D scene, over
 * terrain where the route allows it.
 *
 * @example Load the track
 * ```ts
 * import { loadTrack } from "./app/track";
 *
 * const { scene, flat } = await loadTrack();
 * ```
 */
export async function loadTrack(): Promise<Track> {
  await initWasm()
  const gpx = await (await fetchOk(await gpxUrl())).text()
  const ratio = Math.min(window.devicePixelRatio || 1, config.MAX_PIXEL_RATIO)
  const flatSize = viewport(config.WIDTH, config.HEIGHT, ratio)
  const flat = new GpxRenderer(gpx, flatSize.width, flatSize.height, flatSize.ratio)
  flat.setTailColor(0xff, 0x5a, 0x36)
  // The scene is laid out in CSS pixels; WebGL scales it to the screen.
  const sceneSize = viewport(config.WIDTH, config.HEIGHT, 1)
  const { scene, terrain } = createSceneRenderer(gpx, {
    size: sceneSize,
    pitchDeg: config.PITCH_DEG,
    bearingDeg: config.BEARING_DEG,
    orbitDeg: config.ORBIT_DEG,
    terrain: true,
  })
  scene.setMapStyle(config.MAP_STYLE)
  return { gpx, flat, flatSize, scene, sceneSize, terrain }
}
