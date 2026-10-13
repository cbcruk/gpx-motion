import { describe, expect, it } from 'vite-plus/test'
import { createPlayback } from './playback'

describe('createPlayback', () => {
  it('starts without advancing on the first frame', () => {
    const playback = createPlayback(0.1)
    expect(playback.step(5000, 1, 8000)).toEqual({ u: 0, dt: 0, jumped: false })
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
      expect(playback.step(now, 1, 1000).jumped).toBe(false)
    expect(playback.step(960, 1, 1000).jumped).toBe(false)
    const step = playback.step(1010, 1, 1000)
    expect(step.jumped).toBe(true)
    expect(step.u).toBeCloseTo(0.01)
  })

  it('holds progress while paused, but keeps the time flowing', () => {
    const playback = createPlayback(0.1)
    playback.step(0, 1, 1000)
    playback.step(50, 1, 1000)
    playback.paused = true
    const step = playback.step(100, 1, 1000)
    expect(step.u).toBeCloseTo(0.05)
    expect(step.dt).toBeCloseTo(0.05)
    playback.paused = false
    expect(playback.step(150, 1, 1000).u).toBeCloseTo(0.1)
  })

  it('reports a seek as a jump once, and goes on from there', () => {
    const playback = createPlayback(0.1)
    playback.step(0, 1, 1000)
    playback.seek(0.5)
    const step = playback.step(50, 1, 1000)
    expect(step.jumped).toBe(true)
    expect(step.u).toBeCloseTo(0.55)
    expect(playback.step(100, 1, 1000).jumped).toBe(false)
  })

  it('reports a seek while paused, without moving on', () => {
    const playback = createPlayback(0.1)
    playback.paused = true
    playback.seek(0.25)
    expect(playback.step(0, 1, 1000)).toEqual({ u: 0.25, dt: 0, jumped: true })
  })

  it('keeps a seek within the track', () => {
    const playback = createPlayback(0.1)
    playback.seek(1.5)
    expect(playback.u).toBe(1)
    playback.seek(-1)
    expect(playback.u).toBe(0)
  })
})
