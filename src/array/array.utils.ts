/**
 * Reads the element at `index`, throwing when the index is out of range.
 *
 * Indexing an array past its end yields `undefined`; for laid-out wasm data
 * that means a wrong stride or a truncated buffer, so this fails loudly
 * instead of spreading `undefined` (or `NaN`) through the scene.
 *
 * @throws {RangeError} When `index` is not an integer in `0..array.length - 1`.
 *
 * @example Read a field of a laid-out vertex
 * ```ts
 * import { at } from '@/array/array.utils'
 *
 * const track = new Float32Array([1, 2, 3, 4])
 * at(track, 2) // 3
 * ```
 */
export function at<T>(array: ArrayLike<T>, index: number): T {
  if (!Number.isInteger(index) || index < 0 || index >= array.length) {
    throw new RangeError(`index ${index} out of range for length ${array.length}`)
  }
  return array[index] as T
}

/**
 * Reads the value stored under `key`, throwing when it is missing.
 *
 * @throws {Error} When `record` has no value under `key`.
 *
 * @example Read a shader uniform that must exist
 * ```ts
 * import { entry } from '@/array/array.utils'
 *
 * const uniforms: Record<string, { value: number }> = { uTail: { value: 0 } }
 * entry(uniforms, 'uTail').value = 1
 * ```
 */
export function entry<T>(record: Readonly<Record<string, T>>, key: string): T {
  const value = record[key]
  if (value === undefined) throw new Error(`missing entry: ${key}`)
  return value
}
