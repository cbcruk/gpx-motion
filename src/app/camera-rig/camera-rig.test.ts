import { describe, expect, it } from 'vite-plus/test'
import { initialViewParams } from '../../config/config'
import { createCameraRig } from './camera-rig'
import type { ViewParams } from '../../controls/controls.types'
import type { CameraRig, RigScene } from './camera-rig.types'

/** A rig over a scene whose track is `lengthPx` long on screen at any zoom. */
function rig(lengthPx = 0): {
  rig: CameraRig
  params: ViewParams
  scene: RigScene
  turned: number[]
  follows: number[]
} {
  const params = initialViewParams()
  const turned: number[] = []
  const follows: number[] = []
  const zooms: { follow: number | undefined; framing: number } = { follow: undefined, framing: 0 }
  const scene: RigScene = {
    get followZoom() {
      return zooms.follow
    },
    get framingZoom() {
      return zooms.framing
    },
    setFollow: (zoom) => {
      zooms.follow = zoom ?? undefined
    },
    setFramingZoom: (zoom) => {
      zooms.framing = zoom
    },
    trackLengthPx: () => lengthPx,
  }
  const created = createCameraRig({
    scene,
    params,
    chase: { turnAt: () => 0, turnBy: (deg) => turned.push(deg), snap: () => {} },
    onFollow: () => follows.push(params.zoom),
  })
  return { rig: created, params, scene, turned, follows }
}

describe('createCameraRig', () => {
  it('turns the bearing within 0..360 and turns the chase camera too', () => {
    const { rig: r, params, turned } = rig()
    r.turn(-30)
    expect(params.rotate).toBe(330)
    expect(turned).toEqual([-30])
  })

  it('orbits only when Orbit is on, without turning the chase camera', () => {
    const { rig: r, params, turned } = rig()
    r.orbit(1)
    expect(params.rotate).toBe(0)
    params.orbit = true
    r.orbit(1)
    expect(params.rotate).toBe(12)
    expect(turned).toEqual([])
  })

  it('tilts within 0..75', () => {
    const { rig: r, params } = rig()
    r.tilt(100)
    expect(params.pitch).toBe(75)
    r.tilt(-200)
    expect(params.pitch).toBe(0)
  })

  it('zooms the framing unless following', () => {
    const { rig: r, scene, params } = rig()
    r.zoom(1.5)
    expect(scene.framingZoom).toBe(1.5)
    expect(params.zoom).toBe(16)
  })

  it('zooms a followed marker within its range, slowing the loop to keep pace', () => {
    const { rig: r, params, follows } = rig(4800)
    params.follow = true
    r.applyFollow()
    expect(r.loopMs).toBe(20_000)
    r.zoom(5)
    expect(params.zoom).toBe(18)
    expect(follows).toEqual([16, 18])
  })

  it('keeps the loop when not following', () => {
    const { rig: r } = rig(4800)
    r.applyFollow()
    expect(r.loopMs).toBe(8000)
  })

  it('resets the camera', () => {
    const { rig: r, params, scene } = rig()
    r.turn(40)
    r.tilt(10)
    r.zoom(2)
    r.reset()
    expect([params.rotate, params.pitch, scene.framingZoom, params.zoom]).toEqual([0, 55, 0, 16])
  })
})
