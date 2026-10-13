import { describe, expect, it } from 'vite-plus/test'
import { progressText } from './progress-bar.utils'

describe('progressText', () => {
  it('tells the time elapsed on a timed track', () => {
    expect(progressText(0.5, { durationMs: 3_600_000, lengthM: 8000 })).toBe('30:00 / 1:00:00')
    expect(progressText(0.25, { durationMs: 90_000, lengthM: 8000 })).toBe('0:22 / 1:30')
  })

  it('tells the distance covered on an untimed track', () => {
    expect(progressText(0.5, { durationMs: undefined, lengthM: 8000 })).toBe('4.00 / 8.00 km')
    expect(progressText(1, { durationMs: undefined, lengthM: 3150 })).toBe('3.15 / 3.15 km')
  })
})
