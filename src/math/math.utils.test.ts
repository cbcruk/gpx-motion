import { describe, expect, it } from 'vite-plus/test'
import { wrapDeg } from './math.utils'

describe('wrapDeg', () => {
  it('wraps into 0..period', () => {
    expect(wrapDeg(-30, 360)).toBe(330)
    expect(wrapDeg(725, 360)).toBe(5)
    expect(wrapDeg(360, 360)).toBe(0)
  })
})
