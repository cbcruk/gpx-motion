import { afterEach, describe, expect, it, vi } from 'vite-plus/test'
import { createMapStyles } from './map-style'
import type { MapStyleName } from '../controls/controls.types'
import type { MapStyles } from './map-style.types'

const LIBERTY = 'https://example.com/liberty'

/** A pending response the test settles itself. */
function deferred(): {
  promise: Promise<Response>
  resolve: (body: string, status?: number) => void
} {
  let settle!: (response: Response) => void
  const promise = new Promise<Response>((r) => (settle = r))
  return { promise, resolve: (body, status = 200) => settle(new Response(body, { status })) }
}

function styles(): { styles: MapStyles; applied: string[]; reverted: MapStyleName[] } {
  const applied: string[] = []
  const reverted: MapStyleName[] = []
  const styles = createMapStyles({
    initial: 'dark',
    documents: { liberty: LIBERTY },
    apply: (style) => {
      if (style === 'not a style') throw new Error('unreadable style')
      applied.push(style)
    },
    revert: (name) => reverted.push(name),
  })
  return { styles, applied, reverted }
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('createMapStyles', () => {
  it('applies a preset by name without fetching', async () => {
    const fetch = vi.fn()
    vi.stubGlobal('fetch', fetch)
    const { styles: s, applied } = styles()
    expect(await s.switchTo('light')).toBe(true)
    expect(applied).toEqual(['light'])
    expect([s.name, s.style]).toEqual(['light', 'light'])
    expect(fetch).not.toHaveBeenCalled()
  })

  it('fetches a style document once and keeps it', async () => {
    const fetch = vi.fn(async () => new Response('{"version":8}'))
    vi.stubGlobal('fetch', fetch)
    const { styles: s, applied } = styles()
    expect(await s.switchTo('liberty')).toBe(true)
    await s.switchTo('dark')
    expect(await s.switchTo('liberty')).toBe(true)
    expect(fetch).toHaveBeenCalledTimes(1)
    expect(fetch).toHaveBeenCalledWith(LIBERTY, { signal: null })
    expect(applied).toEqual(['{"version":8}', 'dark', '{"version":8}'])
    expect(s.style).toBe('{"version":8}')
  })

  it('ignores a switch a later one overtook', async () => {
    const response = deferred()
    vi.stubGlobal('fetch', () => response.promise)
    const { styles: s, applied, reverted } = styles()
    const slow = s.switchTo('liberty')
    expect(await s.switchTo('light')).toBe(true)
    response.resolve('{"version":8}')
    expect(await slow).toBe(false)
    expect(applied).toEqual(['light'])
    expect(reverted).toEqual([])
    expect(s.name).toBe('light')
  })

  it('does not revert for an overtaken switch that fails', async () => {
    const response = deferred()
    vi.stubGlobal('fetch', () => response.promise)
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { styles: s, reverted } = styles()
    const slow = s.switchTo('liberty')
    await s.switchTo('light')
    response.resolve('gone', 404)
    expect(await slow).toBe(false)
    expect(reverted).toEqual([])
  })

  it('keeps the current style and reverts the picker when the fetch fails', async () => {
    vi.stubGlobal('fetch', async () => new Response('gone', { status: 404 }))
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { styles: s, applied, reverted } = styles()
    await s.switchTo('light')
    expect(await s.switchTo('liberty')).toBe(false)
    expect(applied).toEqual(['light'])
    expect(reverted).toEqual(['light'])
    expect([s.name, s.style]).toEqual(['light', 'light'])
    expect(warn).toHaveBeenCalledOnce()
  })

  it('reverts and does not keep a document the renderer cannot read', async () => {
    const fetch = vi.fn(async () => new Response('not a style'))
    vi.stubGlobal('fetch', fetch)
    vi.spyOn(console, 'warn').mockImplementation(() => {})
    const { styles: s, reverted } = styles()
    expect(await s.switchTo('liberty')).toBe(false)
    expect(await s.switchTo('liberty')).toBe(false)
    expect(reverted).toEqual(['dark', 'dark'])
    expect(fetch).toHaveBeenCalledTimes(2)
    expect(s.name).toBe('dark')
  })
})
