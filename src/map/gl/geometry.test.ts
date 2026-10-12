import { describe, expect, it } from 'vite-plus/test'
import { curtainGeometry, lineGeometry } from './geometry'

/** A track of `n` vertices: point `(i, 0, 1, 0)`, progress `i / (n - 1)`, color `(i, i, i)`. */
const track = (n: number): Float32Array =>
  new Float32Array(
    Array.from({ length: n }, (_, i) => [i, 0, 1, 0, i / Math.max(1, n - 1), i, i, i]).flat(),
  )

const values = (geometry: ReturnType<typeof lineGeometry>, name: string): number[] =>
  Array.from(geometry.getAttribute(name).array)

describe('lineGeometry', () => {
  it('lays four corners and two triangles per segment', () => {
    const line = lineGeometry(track(3))
    expect(line.getAttribute('aStart').count).toBe(8)
    expect(Array.from(line.getIndex()!.array)).toEqual([0, 1, 2, 2, 1, 3, 4, 5, 6, 6, 5, 7])
    expect(values(line, 'aCorner').slice(0, 8)).toEqual([0, -1, 0, 1, 1, -1, 1, 1])
    expect(values(line, 'aProgress')).toEqual([0, 0, 0.5, 0.5, 0.5, 0.5, 1, 1])
  })

  it('colors each segment by its end vertex', () => {
    const colors = values(lineGeometry(track(3)), 'aColor')
    expect(colors.slice(0, 3)).toEqual([1, 1, 1])
    expect(colors.slice(12, 15)).toEqual([2, 2, 2])
  })

  it('is empty for fewer than two vertices', () => {
    expect(lineGeometry(track(1)).getAttribute('aStart').count).toBe(0)
    expect(lineGeometry(new Float32Array(0)).getIndex()!.count).toBe(0)
  })
})

describe('curtainGeometry', () => {
  it('raises one quad per segment from the ground to the line', () => {
    const curtain = curtainGeometry(track(3))
    expect(curtain.getAttribute('aPoint').count).toBe(8)
    expect(values(curtain, 'aTop')).toEqual([0, 0, 1, 1, 0, 0, 1, 1])
    expect(Array.from(curtain.getIndex()!.array)).toEqual([0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7])
    expect(values(curtain, 'aPoint').slice(0, 2 * 4)).toEqual([0, 0, 1, 0, 1, 0, 1, 0])
    expect(values(curtain, 'aColor').slice(9, 12)).toEqual([1, 1, 1])
  })
})
