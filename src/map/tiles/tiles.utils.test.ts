import { describe, expect, it } from 'vite-plus/test'
import { tileKey } from './tiles.utils'

describe('tileKey', () => {
  it('joins zoom, column and row with slashes', () => {
    expect(tileKey(14, 13970, 6344)).toBe('14/13970/6344')
  })

  it('keeps tiles that differ only in order apart', () => {
    expect(tileKey(1, 2, 3)).not.toBe(tileKey(1, 3, 2))
  })
})
