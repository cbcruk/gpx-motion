import { describe, expect, it } from 'vite-plus/test'
import { createChaseCamera } from './chase-camera'
import type { ChaseCamera, ChaseFrame } from './playback.types'

/** A camera over a route heading `heading(u)` degrees, starting its orbit at 0°. */
function chaseOver(heading: (u: number) => number | undefined): ChaseCamera {
  return createChaseCamera({
    track: { trackLengthM: 1000, headingAt: (u) => heading(u) },
    bearingDeg: 0,
    orbitDeg: 360,
    degPerS: 90,
    headingWindow: { seconds: 2, minM: 150 },
  })
}

const frame = (frame: Partial<ChaseFrame>): ChaseFrame => ({
  u: 0.5,
  dt: 0.1,
  rotate: 0,
  chasing: true,
  speed: 1,
  loopMs: 8000,
  ...frame,
})

/** The compass bearing an orbit turn starting at 0° faces. */
const deg = (turn: number): number => turn * 360

describe('createChaseCamera', () => {
  it('faces the turned bearing when not chasing', () => {
    const chase = chaseOver(() => 90)
    expect(deg(chase.turnAt(frame({ chasing: false, rotate: 30 })))).toBeCloseTo(30)
  })

  it('eases toward the direction of travel at degPerS times the speed', () => {
    const chase = chaseOver(() => 90)
    expect(deg(chase.turnAt(frame({ dt: 0.1 })))).toBeCloseTo(9)
    expect(deg(chase.turnAt(frame({ dt: 0.1, speed: 2 })))).toBeCloseTo(27)
  })

  it('faces the way of travel at once after a snap (the loop starting over)', () => {
    const chase = chaseOver(() => 90)
    chase.turnAt(frame({}))
    chase.snap()
    expect(deg(chase.turnAt(frame({})))).toBeCloseTo(90)
  })

  it('adds the turned bearing to the direction of travel', () => {
    const chase = chaseOver(() => 90)
    chase.snap()
    expect(deg(chase.turnAt(frame({ rotate: 45 })))).toBeCloseTo(135)
  })

  it('holds its bearing where the direction is unknown', () => {
    let heading: number | undefined = 90
    const chase = chaseOver(() => heading)
    chase.snap()
    chase.turnAt(frame({}))
    heading = undefined
    expect(deg(chase.turnAt(frame({})))).toBeCloseTo(90)
  })

  it('eases back to the turned bearing, then lets it go', () => {
    const chase = chaseOver(() => 90)
    chase.snap()
    chase.turnAt(frame({}))
    expect(deg(chase.turnAt(frame({ chasing: false, dt: 0.5 })))).toBeCloseTo(45)
    expect(deg(chase.turnAt(frame({ chasing: false, dt: 0.5 })))).toBeCloseTo(0)
    expect(deg(chase.turnAt(frame({ chasing: false, rotate: 60 })))).toBeCloseTo(60)
  })

  it('turns by a drag at once while chasing', () => {
    const chase = chaseOver(() => 90)
    chase.snap()
    chase.turnAt(frame({}))
    chase.turnBy(10)
    expect(deg(chase.turnAt(frame({ dt: 0, rotate: 10 })))).toBeCloseTo(100)
  })

  it('ignores a drag when not chasing', () => {
    const chase = chaseOver(() => 90)
    chase.turnBy(10)
    expect(deg(chase.turnAt(frame({ chasing: false })))).toBeCloseTo(0)
  })

  it('keeps turning the same way through a U-turn', () => {
    let heading = 0
    const chase = chaseOver(() => heading)
    chase.snap()
    chase.turnAt(frame({}))
    heading = 270
    expect(deg(chase.turnAt(frame({ dt: 0.1 })))).toBeCloseTo(351)
    heading = 170
    // Nearly behind now, but it was turning counter-clockwise: it goes on that way.
    expect(deg(chase.turnAt(frame({ dt: 0.1 })))).toBeCloseTo(342)
  })

  it('widens the heading window with the pace', () => {
    const windows: number[] = []
    const chase = createChaseCamera({
      track: {
        trackLengthM: 8000,
        headingAt: (_u, windowM) => {
          windows.push(windowM)
          return 0
        },
      },
      bearingDeg: 0,
      orbitDeg: 360,
      degPerS: 90,
      headingWindow: { seconds: 2, minM: 150 },
    })
    chase.turnAt(frame({ speed: 1 }))
    chase.turnAt(frame({ speed: 0.01 }))
    expect(windows).toEqual([2000, 150])
  })
})
