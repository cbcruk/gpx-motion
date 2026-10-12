/** A lazily loaded GPX file's URL, as `import.meta.glob` lists it. */
export type SampleLoader = () => Promise<string>
