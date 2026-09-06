import { DatabaseAdapterError } from './adapters.js'

/**
 * Refuse to start without the private environment a database needs.
 *
 * Call it where a process begins — `register()` in `instrumentation.ts`, or the
 * module that creates the client — so a deployment missing `DATABASE_URL`
 * fails at startup with the names listed, not on the first query. A
 * `RUVYXA_PUBLIC_` name is refused outright: that prefix ships to browsers.
 */
export function requireDatabaseEnv(names: readonly string[]): void {
  const unique = [...new Set(names)]
  for (const [index, name] of unique.entries()) {
    if (!/^[A-Z_][A-Z0-9_]*$/.test(name)) {
      throw new TypeError(`requireDatabaseEnv() names[${index}] is not a valid variable name`)
    }
    if (name.startsWith('RUVYXA_PUBLIC_')) {
      throw new TypeError(`requireDatabaseEnv() refuses public database variable ${name}`)
    }
  }
  const missing = unique.filter((name) => !process.env[name]?.trim())
  if (missing.length > 0) {
    throw new DatabaseAdapterError(
      'RUV3001',
      `missing private database environment variables: ${missing.join(', ')}`,
    )
  }
}
