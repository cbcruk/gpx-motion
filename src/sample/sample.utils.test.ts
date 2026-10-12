import { describe, expect, it } from 'vite-plus/test'
import type { SampleLoader } from './sample.types'
import { byName, pickSample, sampleNames } from './sample.utils'

const loader =
  (url: string): SampleLoader =>
  () =>
    Promise.resolve(url)

describe('byName', () => {
  it('keys files by name without folder or extension', () => {
    const files = byName({
      '../../samples/namsan.gpx': loader('a'),
      '../../samples/local/ride.gpx': loader('b'),
    })
    expect([...files.keys()]).toEqual(['namsan', 'ride'])
  })
})

describe('pickSample', () => {
  const shared = new Map([['namsan', loader('shared')]])
  const local = new Map([
    ['namsan', loader('local namsan')],
    ['ride', loader('local ride')],
  ])

  it('prefers a public sample for a bare name', () => {
    expect(pickSample('namsan', local, shared)).toBe(shared.get('namsan'))
  })

  it('falls back to a personal recording', () => {
    expect(pickSample('ride', local, shared)).toBe(local.get('ride'))
  })

  it('reads local/ names from the personal recordings only', () => {
    expect(pickSample('local/namsan', local, shared)).toBe(local.get('namsan'))
    expect(pickSample('local/missing', local, shared)).toBeNull()
  })

  it('returns null for an unknown name', () => {
    expect(pickSample('missing', local, shared)).toBeNull()
  })
})

describe('sampleNames', () => {
  it('lists public samples first, each group sorted', () => {
    const shared = new Map([
      ['seoul-b', loader('')],
      ['namsan', loader('')],
    ])
    const local = new Map([
      ['b', loader('')],
      ['a', loader('')],
    ])
    expect(sampleNames(local, shared)).toEqual(['namsan', 'seoul-b', 'local/a', 'local/b'])
  })
})
