import { DatabaseAdapterError, DATABASE_OPERATION_KINDS } from './adapters.js'
import type {
  DatabaseAdapter,
  DatabaseClient,
  DatabaseOperationKind,
  DatabaseRecord,
  ModelDelegate,
} from './types.js'

export * from './adapters.js'
export * from './plugin.js'
export type * from './types.js'

/** Create a typed database facade over one production adapter. */
export function createDatabase<TSchema extends { [TKey in keyof TSchema]: DatabaseRecord }>(
  adapterValue: DatabaseAdapter,
): DatabaseClient<TSchema> {
  const adapter = validateAdapter(adapterValue)
  const delegates = new Map<string, ModelDelegate<DatabaseRecord>>()
  const control = {
    $adapter: adapter,
    $connect: async () => adapter.connect?.(),
    $disconnect: async () => adapter.disconnect?.(),
    $transaction: async <TResult>(
      run: (database: DatabaseClient<TSchema>) => Promise<TResult>,
    ): Promise<TResult> => {
      if (typeof run !== 'function') {
        throw new TypeError('database.$transaction() requires an async callback')
      }
      if (!adapter.transaction) {
        throw new DatabaseAdapterError(
          'RUV3003',
          `adapter "${adapter.name}" does not support transactions`,
        )
      }
      return adapter.transaction((transactionAdapter) =>
        run(createDatabase<TSchema>(transactionAdapter)),
      ) as Promise<TResult>
    },
  }

  return new Proxy(control as unknown as DatabaseClient<TSchema>, {
    get(target, property, receiver) {
      if (typeof property !== 'string') return Reflect.get(target, property, receiver)
      if (property === 'then') return undefined
      if (property.startsWith('$')) return Reflect.get(target, property, receiver)
      if (!isModelName(property)) {
        throw new DatabaseAdapterError('RUV3001', `unsafe database model name "${property}"`)
      }
      let delegate = delegates.get(property)
      if (!delegate) {
        delegate = createModelDelegate(adapter, property)
        delegates.set(property, delegate)
      }
      return delegate
    },
  })
}

function createModelDelegate(
  adapter: DatabaseAdapter,
  model: string,
): ModelDelegate<DatabaseRecord> {
  const execute = <TResult>(kind: DatabaseOperationKind, args: unknown): Promise<TResult> => {
    return Promise.resolve().then(() => {
      const normalized = normalizeArgs(kind, args)
      return adapter.execute({ model, kind, args: normalized }) as Promise<TResult>
    })
  }
  const delegate: ModelDelegate<DatabaseRecord> = {
    findMany: (args = {}) => execute('findMany', args),
    findFirst: (args = {}) => execute('findFirst', args),
    findUnique: (args) => execute('findUnique', args),
    create: (args) => execute('create', args),
    createMany: (args) => execute('createMany', args),
    update: (args) => execute('update', args),
    updateMany: (args) => execute('updateMany', args),
    delete: (args) => execute('delete', args),
    deleteMany: (args = {}) => execute('deleteMany', args),
    count: (args = {}) => execute('count', args),
  }
  return Object.freeze(delegate)
}

function normalizeArgs(kind: DatabaseOperationKind, value: unknown): Record<string, unknown> {
  if (!DATABASE_OPERATION_KINDS.includes(kind)) {
    throw new DatabaseAdapterError('RUV3001', `unsupported database operation ${kind}`)
  }
  if (!value || typeof value !== 'object' || Array.isArray(value)) {
    throw new DatabaseAdapterError('RUV3001', `${kind}() expects an options object`)
  }
  const args = value as Record<string, unknown>
  for (const key of ['skip', 'take'] as const) {
    if (args[key] !== undefined) {
      const number = args[key]
      const valid = Number.isSafeInteger(number) && (number as number) >= (key === 'take' ? 1 : 0)
      if (!valid || (key === 'take' && (number as number) > 10_000)) {
        throw new DatabaseAdapterError(
          'RUV3001',
          `${kind}() ${key} must be ${key === 'take' ? 'between 1 and 10000' : 'a non-negative integer'}`,
        )
      }
    }
  }
  if (['findUnique', 'update', 'delete'].includes(kind) && !isNonEmptyObject(args.where)) {
    throw new DatabaseAdapterError('RUV3001', `${kind}() requires a non-empty where clause`)
  }
  if (['create', 'update', 'updateMany'].includes(kind) && !isNonEmptyObject(args.data)) {
    throw new DatabaseAdapterError('RUV3001', `${kind}() requires non-empty data`)
  }
  if (kind === 'createMany' && (!Array.isArray(args.data) || args.data.length === 0)) {
    throw new DatabaseAdapterError('RUV3001', 'createMany() requires a non-empty data array')
  }
  return { ...args }
}

function validateAdapter(adapter: DatabaseAdapter): DatabaseAdapter {
  if (!adapter || typeof adapter !== 'object' || typeof adapter.execute !== 'function') {
    throw new TypeError('createDatabase() requires a database adapter')
  }
  if (typeof adapter.name !== 'string' || adapter.name.trim() === '') {
    throw new TypeError('createDatabase() adapter requires a non-empty name')
  }
  return adapter
}

function isNonEmptyObject(value: unknown): value is Record<string, unknown> {
  return (
    !!value && typeof value === 'object' && !Array.isArray(value) && Object.keys(value).length > 0
  )
}

function isModelName(value: string): boolean {
  return /^[A-Za-z]\w*$/.test(value)
}
