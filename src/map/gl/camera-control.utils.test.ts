import { describe, expect, it } from 'vite-plus/test'
import { turnToward } from './camera-control.utils'

describe('turnToward', () => {
  it('goes the short way across north', () => {
    expect(turnToward(350, 10, 15)).toEqual({ deg: 5, turning: 1 })
    expect(turnToward(10, 350, 15)).toEqual({ deg: 355, turning: -1 })
  })

  it('steps at most maxDeg, whatever its sign', () => {
    expect(turnToward(0, 90, 10)).toEqual({ deg: 10, turning: 1 })
    expect(turnToward(0, 90, -10)).toEqual({ deg: 10, turning: 1 })
    expect(turnToward(0, 5, 10)).toEqual({ deg: 5, turning: 1 })
  })

  it('keeps turning the same way toward a bearing nearly behind', () => {
    expect(turnToward(0, 175, 10)).toEqual({ deg: 10, turning: 1 })
    expect(turnToward(0, 175, 10, -1)).toEqual({ deg: 350, turning: -1 })
    expect(turnToward(0, 160, 10, -1)).toEqual({ deg: 10, turning: 1 })
  })

  it('stays put at the target', () => {
    expect(turnToward(30, 390, 10)).toEqual({ deg: 30, turning: 0 })
  })
})
