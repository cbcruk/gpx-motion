import { afterEach, describe, expect, it, vi } from 'vite-plus/test'
import { assertOk, fetchOk } from './fetch.utils'

describe('assertOk', () => {
  it('returns a successful response', () => {
    const response = new Response('ok', { status: 200 })
    expect(assertOk(response, 'https://x.test/a')).toBe(response)
  })

  it('names the URL and status of a failed one', () => {
    const response = new Response('missing', { status: 404, statusText: 'Not Found' })
    expect(() => assertOk(response, 'https://x.test/a')).toThrow(
      'https://x.test/a: HTTP 404 Not Found',
    )
  })

  it('leaves out an empty status text', () => {
    expect(() => assertOk(new Response(null, { status: 500 }), 'u')).toThrow(/^u: HTTP 500$/)
  })
})

describe('fetchOk', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('fails on an HTTP error status', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(() => Promise.resolve(new Response(null, { status: 503 }))),
    )
    await expect(fetchOk('https://x.test/track.gpx')).rejects.toThrow(
      'https://x.test/track.gpx: HTTP 503',
    )
  })

  it('resolves with a successful response', async () => {
    const fetch = vi.fn((_url: string) => Promise.resolve(new Response('<gpx/>')))
    vi.stubGlobal('fetch', fetch)
    expect(await (await fetchOk('https://x.test/a')).text()).toBe('<gpx/>')
    expect(fetch).toHaveBeenCalledWith('https://x.test/a')
  })
})
