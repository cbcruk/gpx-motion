import { describe, expect, it } from 'vite-plus/test'
import { keepoutRect } from './label-keepout.utils'

describe('keepoutRect', () => {
  it('measures the box from the area, scaled to the canvas', () => {
    const area = { left: 100, top: 50, width: 320 }
    const box = { left: 110, top: 350, width: 100, height: 20 }
    expect(keepoutRect(area, box, 640)).toEqual([20, 600, 200, 40])
  })

  it('has nothing to measure before layout', () => {
    expect(
      keepoutRect({ left: 0, top: 0, width: 0 }, { left: 0, top: 0, width: 0, height: 0 }, 640),
    ).toBeNull()
  })
})
