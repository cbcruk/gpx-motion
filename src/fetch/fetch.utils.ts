/**
 * Returns `response` when it succeeded (`2xx`), and otherwise throws an
 * error naming `url` and the HTTP status, dropping the unread body.
 *
 * @param url The URL fetched, for the error message (`response.url` can be empty).
 *
 * @example Accept only a successful response
 * ```ts
 * import { assertOk } from '@/fetch/fetch.utils'
 *
 * const url = '/samples/namsan.gpx'
 * const response = assertOk(await fetch(url), url)
 * ```
 */
export function assertOk(response: Response, url: string): Response {
  if (response.ok) return response
  void response.body?.cancel()
  const status = [response.status, response.statusText].filter(Boolean).join(' ')
  throw new Error(`${url}: HTTP ${status}`)
}

/**
 * Fetches `url`, failing on an HTTP error status as well as a network error
 * (see {@linkcode assertOk}).
 *
 * @param signal Aborts the request (see {@linkcode isAbort}).
 *
 * @example Fetch a GPX file
 * ```ts
 * import { fetchOk } from '@/fetch/fetch.utils'
 *
 * const gpx = await (await fetchOk('/samples/namsan.gpx')).text()
 * ```
 */
export async function fetchOk(url: string, signal?: AbortSignal): Promise<Response> {
  return assertOk(await fetch(url, { signal: signal ?? null }), url)
}

/**
 * Tells whether `err` is the rejection of a request whose signal aborted,
 * which is no failure to report: whoever aborted no longer wants the result.
 *
 * @example Report only real failures
 * ```ts
 * import { fetchOk, isAbort } from '@/fetch/fetch.utils'
 *
 * declare const signal: AbortSignal
 *
 * await fetchOk('/samples/namsan.gpx', signal).catch((err: unknown) => {
 *   if (!isAbort(err)) console.warn('track failed', err)
 * })
 * ```
 */
export function isAbort(err: unknown): boolean {
  return err instanceof DOMException && err.name === 'AbortError'
}
