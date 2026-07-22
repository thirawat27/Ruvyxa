import type { DatabaseAdapter, DatabaseOperation, DatabaseOperationKind } from './types.js'

export interface PrismaDelegate {
  findMany?(args: unknown): Promise<unknown>
  findFirst?(args: unknown): Promise<unknown>
  findUnique?(args: unknown): Promise<unknown>
  create?(args: unknown): Promise<unknown>
  createMany?(args: unknown): Promise<unknown>
  update?(args: unknown): Promise<unknown>
  updateMany?(args: unknown): Promise<unknown>
  delete?(args: unknown): Promise<unknown>
  deleteMany?(args: unknown): Promise<unknown>
  count?(args: unknown): Promise<unknown>
}

export interface PrismaClientLike {
  $connect?(): Promise<void>
  $disconnect?(): Promise<void>
  $transaction?<TResult>(run: (client: PrismaClientLike) => Promise<TResult>): Promise<TResult>
  [model: string]: unknown
}

export interface PrismaAdapterOptions {
  /** Map public model names such as `users` to Prisma delegates such as `user`. */
  models?: Readonly<Record<string, string>>
}

/** Bridge Prisma-compatible delegates, covering PostgreSQL, MySQL, SQLite, and MongoDB. */
export function prismaAdapter(
  client: PrismaClientLike,
  options: PrismaAdapterOptions = {},
): DatabaseAdapter {
  if (!client || typeof client !== 'object') {
    throw new TypeError('prismaAdapter() requires a Prisma-compatible client object')
  }

  return {
    name: 'prisma',
    async execute(operation: DatabaseOperation): Promise<unknown> {
      const delegateName = options.models?.[operation.model] ?? operation.model
      const delegate = client[delegateName] as PrismaDelegate | undefined
      const method = delegate?.[operation.kind]
      if (typeof method !== 'function') {
        throw new DatabaseAdapterError(
          'RUV3002',
          `Prisma delegate "${delegateName}" does not implement ${operation.kind}()`,
        )
      }
      return Reflect.apply(method, delegate, [operation.args]) as Promise<unknown>
    },
    ...(typeof client.$connect === 'function'
      ? { connect: () => Reflect.apply(client.$connect as () => Promise<void>, client, []) }
      : {}),
    ...(typeof client.$disconnect === 'function'
      ? { disconnect: () => Reflect.apply(client.$disconnect as () => Promise<void>, client, []) }
      : {}),
    ...(typeof client.$transaction === 'function'
      ? {
          transaction: (run: (adapter: DatabaseAdapter) => Promise<unknown>) =>
            Reflect.apply(client.$transaction as Function, client, [
              (transactionClient: PrismaClientLike) =>
                run(prismaAdapter(transactionClient, options)),
            ]) as Promise<unknown>,
        }
      : {}),
  }
}

export interface DynamoOperationTransport {
  /** Execute a normalized database operation with the configured table name. */
  execute(operation: DatabaseOperation & { table: string }): Promise<unknown>
  transaction?(run: (transport: DynamoOperationTransport) => Promise<unknown>): Promise<unknown>
  connect?(): Promise<void>
  disconnect?(): Promise<void>
}

export interface DynamoAdapterOptions {
  transport: DynamoOperationTransport
  /** Explicit model-to-table mapping; unknown models fail closed. */
  tables: Readonly<Record<string, string>>
}

/**
 * Bridge DynamoDB through an explicit transport so AWS SDK v2, v3, local emulators,
 * and signed HTTP implementations can share the same safe Ruvyxa query contract.
 */
export function dynamoAdapter(options: DynamoAdapterOptions): DatabaseAdapter {
  const { transport, tables } = options
  if (!transport || typeof transport.execute !== 'function') {
    throw new TypeError('dynamoAdapter() requires transport.execute(operation)')
  }
  const normalizedTables = Object.fromEntries(
    Object.entries(tables ?? {}).map(([model, table]) => {
      if (!isIdentifier(model) || typeof table !== 'string' || table.trim() === '') {
        throw new TypeError('dynamoAdapter() table mappings require safe model and table names')
      }
      return [model, table.trim()]
    }),
  )

  return {
    name: 'dynamodb',
    execute(operation: DatabaseOperation): Promise<unknown> {
      const table = normalizedTables[operation.model]
      if (!table) {
        throw new DatabaseAdapterError(
          'RUV3002',
          `DynamoDB model "${operation.model}" has no configured table`,
        )
      }
      return transport.execute({ ...operation, table })
    },
    ...(transport.connect ? { connect: () => transport.connect!() } : {}),
    ...(transport.disconnect ? { disconnect: () => transport.disconnect!() } : {}),
    ...(transport.transaction
      ? {
          transaction: (run: (adapter: DatabaseAdapter) => Promise<unknown>) =>
            transport.transaction!((transactionTransport) =>
              run(dynamoAdapter({ transport: transactionTransport, tables: normalizedTables })),
            ),
        }
      : {}),
  }
}

/** Validate and freeze a custom adapter without hiding its driver errors. */
export function defineDatabaseAdapter(adapter: DatabaseAdapter): DatabaseAdapter {
  if (!adapter || typeof adapter !== 'object' || typeof adapter.execute !== 'function') {
    throw new TypeError('defineDatabaseAdapter() requires execute(operation)')
  }
  if (typeof adapter.name !== 'string' || adapter.name.trim() === '') {
    throw new TypeError('Database adapters require a non-empty name')
  }
  return Object.freeze({ ...adapter, name: adapter.name.trim() })
}

export class DatabaseAdapterError extends Error {
  constructor(
    readonly code: 'RUV3001' | 'RUV3002' | 'RUV3003',
    message: string,
    options?: ErrorOptions,
  ) {
    super(`${code} ${message}`, options)
    this.name = 'DatabaseAdapterError'
  }
}

export const DATABASE_OPERATION_KINDS: readonly DatabaseOperationKind[] = Object.freeze([
  'findMany',
  'findFirst',
  'findUnique',
  'create',
  'createMany',
  'update',
  'updateMany',
  'delete',
  'deleteMany',
  'count',
])

function isIdentifier(value: string): boolean {
  return /^[A-Za-z][A-Za-z0-9_]*$/.test(value)
}
