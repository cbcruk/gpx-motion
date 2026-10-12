import sampleGpxUrl from '../../samples/namsan.gpx?url'
import { byName, pickSample, sampleNames } from './sample.utils'

/**
 * Resolves the URL of the GPX file the demo animates.
 *
 * A production build always shows `samples/namsan.gpx`. In development,
 * `?gpx=<name>` picks `samples/<name>.gpx`, and `?gpx=local/<name>` a
 * personal recording in `samples/local/<name>.gpx` (a bare name falls back
 * to one when no public sample has it); without it, the first local
 * recording is shown if there is one. Personal recordings are never
 * bundled.
 *
 * @example Load the chosen track
 * ```ts
 * import { gpxUrl } from '@/sample/sample'
 *
 * const gpx = await fetch(await gpxUrl()).then((r) => r.text())
 * ```
 */
export async function gpxUrl(): Promise<string> {
  if (!import.meta.env.DEV) return sampleGpxUrl
  const local = byName(
    import.meta.glob<string>('../../samples/local/*.gpx', { query: '?url', import: 'default' }),
  )
  const shared = byName(
    import.meta.glob<string>('../../samples/*.gpx', { query: '?url', import: 'default' }),
  )
  const name = new URLSearchParams(location.search).get('gpx') || null
  if (name !== null) {
    const load = pickSample(name, local, shared)
    if (load) return load()
    const names = sampleNames(local, shared).join(', ')
    console.warn(`no GPX sample "${name}"; pick one of: ${names}`)
  }
  const [first] = local.values()
  return first ? first() : sampleGpxUrl
}
