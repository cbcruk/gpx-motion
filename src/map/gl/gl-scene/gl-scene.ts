import {
  BufferGeometry,
  Color,
  DoubleSide,
  Float32BufferAttribute,
  Group,
  GreaterDepth,
  NotEqualStencilFunc,
  ReplaceStencilOp,
  LessEqualDepth,
  LinearSRGBColorSpace,
  Mesh,
  PerspectiveCamera,
  Points,
  Scene,
  ShaderMaterial,
  Vector2,
  Vector4,
  WebGLRenderer,
  type DepthModes,
  type IUniform,
  type Object3D,
} from 'three'
import type { SceneRenderer } from '../../../../crates/wasm/pkg/gpx_motion.js'
import { at, entry } from '../../../array/array.utils'
import { readCamera, readMarker, trackStride } from '../../layout/layout'
import {
  curtainGeometry,
  dotGeometry,
  lineGeometry,
  moveDot,
  moveSegment,
} from '../geometry/geometry'
import type { GlScene } from './gl-scene.types'
import { createGround } from '../ground/ground'
import type { Ground, GroundLoader } from '../ground/ground.types'
import {
  curtainFragment,
  curtainVertex,
  dotFragment,
  dotVertex,
  fogFragment,
  fogVertex,
  lineFragment,
  lineVertex,
} from '../shaders/shaders'

/** Fraction of its distance a fragment keeps for the depth test (1% toward the eye). */
const DEPTH_PULL = 0.99
/** Near and far planes as multiples of the camera's distance to its target. */
const NEAR_FACTOR = 0.02
const FAR_FACTOR = 10
/** Opacity of the track and marker behind a hill, as in the CPU renderer. */
const HIDDEN_ALPHA = 0.35
const CURTAIN_ALPHA = 0.38
const GHOST_CURTAIN = new Vector4(1, 1, 1, 0x14 / 255)
const GHOST_LINE = new Vector4(1, 1, 1, 0x70 / 255)
const SHADOW = new Vector4(0, 0, 0, 0x90 / 255)

/** Reads the `r`, `g`, `b` of a wasm color (`0..=1`) as a three.js color. */
const rgb = (values: Float32Array): Color => new Color(at(values, 0), at(values, 1), at(values, 2))

/** Which part of the line a line material draws (`uMode` in the shader). */
const LineMode = { Ghost: 0, Tail: 1, Solid: 2 } as const
type LineMode = (typeof LineMode)[keyof typeof LineMode]

/** Draw order: opaque terrain first (buildings at 0.5, see `createGround`), then fog, curtain, lines, and the marker on top. */
const Layer = {
  Fog: 1,
  Curtain: 2,
  Ghost: 3,
  HiddenTail: 4,
  Tail: 5,
  Shadow: 6,
  Drop: 7,
  HiddenMarker: 8,
  Marker: 9,
} as const

/**
 * Draws a 3D scene with WebGL through three.js: the ground (map tiles draped
 * over the terrain, see {@linkcode createGround}), the raised track with its
 * curtain, the marker, and the fog, all from `scene`'s data. The track and marker behind hills show faintly, decided by
 * the depth buffer.
 *
 * `canvas` is drawn at `scene`'s size times `pixelRatio`: build `scene` at
 * CSS size (ratio 1), and its strokes and fog are scaled here. Labels and
 * the elevation profile are not drawn here.
 *
 * The scene owns the ground loader it is given and frees it, with its GPU
 * resources, on `dispose`.
 *
 * @example Draw a scene every frame
 * ```ts
 * import type { SceneRenderer } from '@wasm'
 * import { createGlScene } from '@/map/gl/gl-scene/gl-scene'
 * import { createGroundLoader } from '@/map/gl/ground-loader/ground-loader'
 * import type { TileSource } from '@/map/tiles/tiles.types'
 *
 * declare const canvas: HTMLCanvasElement
 * declare const scene: SceneRenderer
 * declare const source: TileSource
 *
 * const view = createGlScene(canvas, scene, 2)
 * view.setGround(createGroundLoader(source, 'dark'))
 * view.setTerrain() // after scene.setTerrainHeights()
 * view.render(0.5, 0)
 * view.dispose()
 * ```
 */
export function createGlScene(
  canvas: HTMLCanvasElement,
  scene: SceneRenderer,
  pixelRatio = 1,
): GlScene {
  const renderer = new WebGLRenderer({ canvas, antialias: true, stencil: true })
  renderer.outputColorSpace = LinearSRGBColorSpace
  renderer.setPixelRatio(pixelRatio)
  renderer.setSize(scene.width, scene.height, false)
  const buffer = renderer.getDrawingBufferSize(new Vector2())
  const background = rgb(scene.glBackground)
  renderer.setClearColor(background, 1)

  /** Every material made here, freed on dispose. */
  const materials: ShaderMaterial[] = []
  const world = new Scene()
  const camera = new PerspectiveCamera(40, scene.width / scene.height)
  const shared = {
    uTerrain: { value: 1 },
    uPull: { value: DEPTH_PULL },
    uResolution: { value: buffer },
    uNear: { value: 1 },
    uU: { value: 0 },
    uTail: { value: 1 },
  } satisfies Record<string, IUniform>
  const material = (
    vertexShader: string,
    fragmentShader: string,
    uniforms: Record<string, unknown>,
    depthFunc: DepthModes = LessEqualDepth,
    stencilRef?: number,
  ): ShaderMaterial => {
    const made = new ShaderMaterial({
      ...(stencilRef !== undefined && {
        stencilWrite: true,
        stencilRef,
        stencilFunc: NotEqualStencilFunc,
        stencilZPass: ReplaceStencilOp,
      }),
      vertexShader,
      fragmentShader,
      uniforms: {
        ...shared,
        ...Object.fromEntries(Object.entries(uniforms).map(([k, value]) => [k, { value }])),
      },
      transparent: true,
      depthWrite: false,
      depthFunc,
      side: DoubleSide,
    })
    materials.push(made)
    return made
  }
  const add = <T extends Object3D>(object: T, layer: number): T => {
    object.renderOrder = layer
    object.frustumCulled = false
    world.add(object)
    return object
  }

  const fogShape = new BufferGeometry()
  fogShape.setAttribute('position', new Float32BufferAttribute([-1, -1, 0, 3, -1, 0, -1, 3, 0], 3))
  const fog = material(fogVertex, fogFragment, {
    uColor: background.clone(),
    uBand: new Vector2(-2, -1),
    uHeight: buffer.y,
  })
  fog.depthTest = false
  add(new Mesh(fogShape, fog), Layer.Fog)

  const width = scene.glTailWidth * pixelRatio
  const lineMaterial = (
    mode: LineMode,
    color: Vector4,
    alpha: number,
    w: number,
    depth?: DepthModes,
    stencilRef?: number,
  ): ShaderMaterial =>
    material(
      lineVertex,
      lineFragment,
      { uMode: mode, uColor: color, uAlpha: alpha, uWidth: w },
      depth,
      stencilRef,
    )
  // Each translucent pass marks the stencil with its own value, so where
  // segments overlap (joins, folds) it blends once, like one stroked path.
  const lines = [
    [lineMaterial(LineMode.Ghost, GHOST_LINE, 1, width * 0.6, undefined, Layer.Ghost), Layer.Ghost],
    [
      lineMaterial(
        LineMode.Tail,
        GHOST_LINE,
        HIDDEN_ALPHA,
        width * 0.6,
        GreaterDepth,
        Layer.HiddenTail,
      ),
      Layer.HiddenTail,
    ],
    [lineMaterial(LineMode.Tail, GHOST_LINE, 1, width, undefined, Layer.Tail), Layer.Tail],
  ] as const
  const curtain = material(
    curtainVertex,
    curtainFragment,
    { uColor: GHOST_CURTAIN, uAlpha: CURTAIN_ALPHA },
    LessEqualDepth,
    Layer.Curtain,
  )
  let trackParts: Mesh[] = []
  const layTrack = (): void => {
    for (const part of trackParts) {
      world.remove(part)
      part.geometry.dispose()
    }
    const track = scene.glTrack()
    const line = lineGeometry(track)
    trackParts = [
      add(new Mesh(curtainGeometry(track), curtain), Layer.Curtain),
      ...lines.map(([m, layer]) => add(new Mesh(line, m), layer)),
    ]
  }
  layTrack()

  const radius = scene.glMarkerRadius * pixelRatio
  const markerColor = new Vector4().fromArray(scene.glMarkerColor)
  const fade = (alpha: number): Vector4 => markerColor.clone().setW(markerColor.w * alpha)
  const marker = dotGeometry()
  const dot = (size: number, color: Vector4, top: number): ShaderMaterial =>
    material(dotVertex, dotFragment, { uSize: size, uColor: color, uTop: top })
  // Whether terrain hides the marker is decided for all of it (by the core,
  // as the CPU renderer does), so the dot itself skips the depth test.
  const shown = (m: ShaderMaterial): ShaderMaterial => {
    m.depthTest = false
    return m
  }
  const shadow = add(new Points(marker, dot(radius * 1.4, SHADOW, 0)), Layer.Shadow)
  const hiddenColor = fade(HIDDEN_ALPHA)
  const hiddenDot = add(
    new Points(marker, shown(dot(radius * 2, hiddenColor, 1))),
    Layer.HiddenMarker,
  )
  const dotMesh = add(new Points(marker, shown(dot(radius * 2, markerColor, 1))), Layer.Marker)
  const dropColor = fade(0.7)
  const drop = lineGeometry(new Float32Array(2 * trackStride()))
  const dropMesh = add(
    new Mesh(drop, lineMaterial(LineMode.Solid, dropColor, 1, radius / 4)),
    Layer.Drop,
  )

  /** Takes the map style's background and fog color, returning it for the ground's fade. */
  const setBackground = (): Color => {
    const background = rgb(scene.glBackground)
    renderer.setClearColor(background, 1)
    ;(entry(fog.uniforms, 'uColor').value as Color).copy(background)
    return background
  }
  const groundGroup = new Group()
  world.add(groundGroup)
  let loader: GroundLoader | null = null
  let ground: Ground | null = null
  let disposed = false
  /** Replaces the ground with one drawn by `loader` (if any), faded to `background`. */
  const layGround = (background: Color): void => {
    ground?.dispose()
    ground =
      loader &&
      createGround(groundGroup, scene, loader, {
        pixelRatio,
        background,
        anisotropy: renderer.capabilities.getMaxAnisotropy(),
        terrain: shared.uTerrain,
      })
  }
  return {
    setGround(next): void {
      if (disposed) {
        next.dispose()
        return
      }
      if (loader !== next) loader?.dispose()
      loader = next
      layGround(setBackground())
    },
    setMapStyle(style): void {
      if (disposed) return
      const background = setBackground()
      if (!loader) return
      loader.setStyle(style)
      layGround(background)
    },
    setTerrain(): void {
      if (disposed) return
      ground?.reset()
      layTrack()
    },
    setTail(share): void {
      shared.uTail.value = share
    },
    setTerrainVisible(visible): void {
      shared.uTerrain.value = visible ? 1 : 0
    },
    render(u, turn): void {
      if (disposed) return
      const view = readCamera(scene.glCamera(turn, u))
      camera.fov = view.fov
      camera.near = view.distance * NEAR_FACTOR
      camera.far = view.distance * FAR_FACTOR
      camera.position.set(view.eyeX, view.eyeH, view.eyeY)
      // Up the screen is the bearing's direction on the ground, which also
      // holds when looking straight down (where world up would be undefined).
      const b = (view.bearing * Math.PI) / 180
      camera.up.set(Math.sin(b), 0, -Math.cos(b))
      camera.lookAt(view.targetX, 0, view.targetY)
      camera.updateProjectionMatrix()
      shared.uNear.value = camera.near
      shared.uU.value = u
      entry(fog.uniforms, 'uBand').value.set(
        view.fog ? (view.fog.horizon * buffer.y) / scene.height : -2,
        view.fog ? (view.fog.end * buffer.y) / scene.height : -1,
      )

      const at = readMarker(scene.glMarker(u, turn))
      hiddenDot.visible = at.hidden
      for (const part of [shadow, dotMesh, dropMesh]) part.visible = !at.hidden
      moveDot(marker, at)
      moveSegment(drop, { ...at, lift: 0 }, at)
      ground?.update(turn, u, view.far)
      renderer.render(world, camera)
    },
    dispose(): void {
      if (disposed) return
      disposed = true
      ground?.dispose()
      ground = null
      loader?.dispose()
      loader = null
      for (const part of trackParts) part.geometry.dispose()
      for (const geometry of [fogShape, marker, drop]) geometry.dispose()
      for (const m of materials) m.dispose()
      world.clear()
      renderer.dispose()
    },
  }
}
