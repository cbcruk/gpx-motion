/** A lazily loaded GPX file's URL, as `import.meta.glob` lists it. */
export type SampleLoader = () => Promise<string>;

/**
 * Keys `import.meta.glob` results by file name without the folder or
 * `.gpx`, e.g. `../samples/namsan.gpx` as `namsan`.
 */
export function byName(files: Record<string, SampleLoader>): Map<string, SampleLoader> {
  return new Map(
    Object.entries(files).map(([path, load]) => [
      path.replace(/^.*\//, "").replace(/\.gpx$/i, ""),
      load,
    ]),
  );
}

/**
 * Picks the sample `name` asks for: `local/<name>` is a personal recording,
 * and a bare name is a public sample, or a personal recording when no
 * public sample has that name.
 *
 * @returns The sample's loader, or `null` when `name` matches none.
 */
export function pickSample(
  name: string,
  local: Map<string, SampleLoader>,
  shared: Map<string, SampleLoader>,
): SampleLoader | null {
  if (name.startsWith("local/")) return local.get(name.slice("local/".length)) ?? null;
  return shared.get(name) ?? local.get(name) ?? null;
}

/** The names {@linkcode pickSample} accepts, public samples first. */
export function sampleNames(
  local: Map<string, SampleLoader>,
  shared: Map<string, SampleLoader>,
): string[] {
  return [...[...shared.keys()].sort(), ...[...local.keys()].sort().map((n) => `local/${n}`)];
}
