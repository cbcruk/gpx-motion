import { describe, expect, it } from 'vite-plus/test'
import { advance, loopMsFor, orbitTurn, sameBearing } from './playback.utils'

describe('advance', () => {
  it('moves on by the share of a loop that passed', () => {
    expect(advance(0.25, 2000, 1, 8000)).toEqual({ u: 0.5, wrapped: false })
  })

  it('scales the step by the speed', () => {
    expect(advance(0, 1000, 2, 8000).u).toBeCloseTo(0.25)
    expect(advance(0, 1000, 0.5, 8000).u).toBeCloseTo(0.0625)
  })

  it('wraps around at the end of the loop', () => {
    const step = advance(0.9, 1000, 2, 8000)
    expect(step.wrapped).toBe(true)
    expect(step.u).toBeCloseTo(0.15)
  })

  it('wraps exactly at the end', () => {
    expect(advance(0.5, 4000, 1, 8000)).toEqual({ u: 0, wrapped: true })
  })

  it('stays put without time passing', () => {
    expect(advance(0.3, 0, 4, 8000)).toEqual({ u: 0.3, wrapped: false })
  })
})

describe('sameBearing', () => {
  it('compares the short way round', () => {
    expect(sameBearing(359.995, 0, 0.01)).toBe(true)
    expect(sameBearing(10, 370, 0.01)).toBe(true)
    expect(sameBearing(10, 11, 0.01)).toBe(false)
  })
})

describe('orbitTurn', () => {
  it('measures the bearing from the start of the orbit', () => {
    expect(orbitTurn(20, 20, 360)).toBe(0)
    expect(orbitTurn(110, 20, 360)).toBe(0.25)
    expect(orbitTurn(-70, 20, 360)).toBe(0.75)
  })
})

describe('loopMsFor', () => {
  it('keeps the loop when not following', () => {
    expect(loopMsFor(undefined, 8000, 240)).toBe(8000)
  })

  it('never goes faster than the loop', () => {
    expect(loopMsFor(240, 8000, 240)).toBe(8000)
  })

  it('slows down so a followed marker keeps its screen pace', () => {
    expect(loopMsFor(4800, 8000, 240)).toBe(20000)
  })
})
