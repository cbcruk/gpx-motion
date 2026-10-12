/**
 * Type-checks the ```ts code blocks in the JSDoc comments under `src/`.
 *
 * Each block becomes its own module in `node_modules/.cache/docs-check/`,
 * checked by `tsc` with the project's `tsconfig.json` (so `@/` imports
 * resolve as in the app); errors are reported at the block's line in the
 * source file. Blocks must import what they use and declare their inputs
 * (`declare const scene: SceneRenderer`).
 *
 * Run with `vp run docs:check`.
 *
 * @module
 */
import { spawnSync } from 'node:child_process'
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join, relative } from 'node:path'

/** A code block taken from a JSDoc comment. */
interface Example {
  /** The source file, relative to the repository root. */
  file: string
  /** The 1-based line in `file` of the block's first code line. */
  line: number
  /** The block's code. */
  code: string
}

const root = join(import.meta.dirname, '..')
const outDir = join(root, 'node_modules', '.cache', 'docs-check')

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name)
    if (entry.isDirectory()) return sourceFiles(path)
    return entry.name.endsWith('.ts') ? [path] : []
  })
}

function examplesOf(path: string): Example[] {
  const file = relative(root, path)
  const lines = readFileSync(path, 'utf8').split('\n')
  const examples: Example[] = []
  let inDoc = false
  let block: Example | null = null
  lines.forEach((raw, i) => {
    const text = raw.trim()
    if (!inDoc) {
      inDoc = text.startsWith('/**') && !text.endsWith('*/')
      return
    }
    if (text.startsWith('*/') || text.endsWith('*/')) {
      inDoc = false
      block = null
      return
    }
    const body = raw.replace(/^\s*\* ?/, '')
    if (block === null) {
      if (/^```(ts|typescript)\s*$/.test(body.trim())) {
        block = { file, line: i + 2, code: '' }
        examples.push(block)
      }
    } else if (body.trim() === '```') {
      block = null
    } else {
      block.code += `${body}\n`
    }
  })
  return examples
}

const examples = sourceFiles(join(root, 'src')).flatMap(examplesOf)

rmSync(outDir, { recursive: true, force: true })
mkdirSync(outDir, { recursive: true })
const byName = new Map<string, Example>()
examples.forEach((example, i) => {
  const name = `example-${i}.ts`
  byName.set(name, example)
  writeFileSync(join(outDir, name), example.code)
})
writeFileSync(
  join(outDir, 'tsconfig.json'),
  JSON.stringify({
    extends: relative(outDir, join(root, 'tsconfig.json')),
    compilerOptions: { noUnusedLocals: false, noEmit: true },
    include: ['*.ts'],
  }),
)

const tsc = spawnSync(
  process.execPath,
  [join(root, 'node_modules', 'typescript', 'bin', 'tsc'), '-p', outDir, '--pretty', 'false'],
  { encoding: 'utf8' },
)
const output = `${tsc.stdout}${tsc.stderr}`.replace(
  /^.*?(example-\d+\.ts)\((\d+),(\d+)\)/gm,
  (match: string, name: string, line: string, column: string) => {
    const example = byName.get(name)
    if (!example) return match
    return `${example.file}(${example.line + Number(line) - 1},${column})`
  },
)
if (output.trim()) console.log(output.trimEnd())
if (tsc.status !== 0) {
  console.error(`docs:check failed (${examples.length} examples checked)`)
  process.exit(tsc.status ?? 1)
}
console.log(`docs:check: ${examples.length} examples type-check`)
