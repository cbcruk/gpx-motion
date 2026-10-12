import { describe, expect, it } from 'vite-plus/test'
import { at, entry } from './array.utils'

describe('at', () => {
  it('reads elements in range', () => {
    const values = new Float32Array([1, 2, 3])
    expect(at(values, 0)).toBe(1)
    expect(at(values, 2)).toBe(3)
    expect(at(['a', 'b'], 1)).toBe('b')
  })

  it('throws out of range', () => {
    const values = new Uint32Array(2)
    expect(() => at(values, 2)).toThrow(RangeError)
    expect(() => at(values, -1)).toThrow(RangeError)
    expect(() => at(values, 0.5)).toThrow(RangeError)
  })

  it('keeps NaN stored in the array', () => {
    expect(at(new Float64Array([Number.NaN]), 0)).toBeNaN()
  })
})

describe('entry', () => {
  it('reads present keys and throws on missing ones', () => {
    const record: Record<string, number> = { a: 1 }
    expect(entry(record, 'a')).toBe(1)
    expect(() => entry(record, 'b')).toThrow('missing entry: b')
  })
})
