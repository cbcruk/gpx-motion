import { describe, expect, it } from 'vite-plus/test'
import { createPlayback } from './playback'

describe('createPlayback', () => {
  it('starts without advancing on the first frame', () => {
    const playback = createPlayback(0.1)
    expect(playback.step(5000, 1, 8000)).toEqual({ u: 0, dt: 0, wrapped: false })
  })

  it('advances by the time between frames, at the speed set', () => {
    const playback = createPlayback(0.1)
    playback.step(0, 1, 1000)
    const step = playback.step(50, 2, 1000)
    expect(step.dt).toBeCloseTo(0.05)
    expect(step.u).toBeCloseTo(0.1)
    expect(playback.u).toBeCloseTo(0.1)
  })

  it('caps a long gap between frames', () => {
    const playback = createPlayback(0.1)
    playback.step(0, 1, 1000)
    const step = playback.step(10_000, 1, 1000)
    expect(step.dt).toBe(0.1)
    expect(step.u).toBeCloseTo(0.1)
  })

  it('reports starting over', () => {
    const playback = createPlayback(0.1)
    playback.step(0, 1, 1000)
    for (let now = 100; now < 1000; now += 100)
      expect(playback.step(now, 1, 1000).wrapped).toBe(false)
    expect(playback.step(960, 1, 1000).wrapped).toBe(false)
    const step = playback.step(1010, 1, 1000)
    expect(step.wrapped).toBe(true)
    expect(step.u).toBeCloseTo(0.01)
  })
})
