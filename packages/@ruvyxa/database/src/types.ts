export type DatabaseRecord = Record<string, unknown>
export type DatabaseSchema = Record<string, DatabaseRecord>
export type SortDirection = 'asc' | 'desc'

export interface ScalarFilter<TValue> {
  equals?: TValue
  not?: TValue | ScalarFilter<TValue>
  in?: readonly TValue[]
  notIn?: readonly TValue[]
  lt?: TValue
  lte?: TValue
  gt?: TValue
  gte?: TValue
  contains?: TValue extends string ? string : never
  startsWith?: TValue extends string ? string : never
  endsWith?: TValue extends string ? string : never
}

export type Where<TRecord extends DatabaseRecord> = {
  [TKey in keyof TRecord]?: TRecord[TKey] | ScalarFilter<TRecord[TKey]>
} & {
  AND?: readonly Where<TRecord>[]
  OR?: readonly Where<TRecord>[]
  NOT?: Where<TRecord> | readonly Where<TRecord>[]
}

export type OrderBy<TRecord extends DatabaseRecord> = Partial<
  Record<Extract<keyof TRecord, string>, SortDirection>
>

export type Select<TRecord extends DatabaseRecord> = Partial<
  Record<Extract<keyof TRecord, string>, boolean>
>

export interface FindManyArgs<TRecord extends DatabaseRecord> {
  where?: Where<TRecord>
  orderBy?: OrderBy<TRecord> | readonly OrderBy<TRecord>[]
  skip?: number
  take?: number
  select?: Select<TRecord>
  /** Adapter-specific relation graph, such as Prisma `include`. */
  include?: Record<string, unknown>
}

export interface FindOneArgs<TRecord extends DatabaseRecord> {
  where: Where<TRecord>
  select?: Select<TRecord>
  include?: Record<string, unknown>
}

export interface CreateArgs<TRecord extends DatabaseRecord> {
  data: Partial<TRecord>
  select?: Select<TRecord>
  include?: Record<string, unknown>
}

export interface UpdateArgs<TRecord extends DatabaseRecord> extends CreateArgs<TRecord> {
  where: Where<TRecord>
}

export interface DeleteArgs<TRecord extends DatabaseRecord> {
  where: Where<TRecord>
  select?: Select<TRecord>
  include?: Record<string, unknown>
}

export type DatabaseOperationKind =
  | 'findMany'
  | 'findFirst'
  | 'findUnique'
  | 'create'
  | 'createMany'
  | 'update'
  | 'updateMany'
  | 'delete'
  | 'deleteMany'
  | 'count'

export interface DatabaseOperation {
  model: string
  kind: DatabaseOperationKind
  args: Record<string, unknown>
}

export interface DatabaseAdapter {
  readonly name: string
  execute(operation: DatabaseOperation): Promise<unknown>
  transaction?(run: (adapter: DatabaseAdapter) => Promise<unknown>): Promise<unknown>
  connect?(): Promise<void>
  disconnect?(): Promise<void>
}

export interface ModelDelegate<TRecord extends DatabaseRecord> {
  findMany(args?: FindManyArgs<TRecord>): Promise<TRecord[]>
  findFirst(args?: FindManyArgs<TRecord>): Promise<TRecord | null>
  findUnique(args: FindOneArgs<TRecord>): Promise<TRecord | null>
  create(args: CreateArgs<TRecord>): Promise<TRecord>
  createMany(args: { data: readonly Partial<TRecord>[] }): Promise<{ count: number }>
  update(args: UpdateArgs<TRecord>): Promise<TRecord>
  updateMany(args: { where?: Where<TRecord>; data: Partial<TRecord> }): Promise<{ count: number }>
  delete(args: DeleteArgs<TRecord>): Promise<TRecord>
  deleteMany(args?: { where?: Where<TRecord> }): Promise<{ count: number }>
  count(args?: { where?: Where<TRecord> }): Promise<number>
}

export type DatabaseClient<TSchema extends { [TKey in keyof TSchema]: DatabaseRecord }> = {
  [TModel in keyof TSchema]: ModelDelegate<TSchema[TModel]>
} & {
  $adapter: DatabaseAdapter
  $connect(): Promise<void>
  $disconnect(): Promise<void>
  $transaction<TResult>(
    run: (database: DatabaseClient<TSchema>) => Promise<TResult>,
  ): Promise<TResult>
}
