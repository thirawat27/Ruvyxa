export interface LoaderContext {
  params: Record<string, string>
  request: Request
  cache: typeof cache
}

export interface ActionContext<TInput> {
  input: TInput
  request: Request
  user?: unknown
  invalidate(key: string): void
}

export type LoaderHandler<TResult> = (ctx: LoaderContext) => TResult | Promise<TResult>

export interface Loader<TResult> {
  (ctx?: Partial<LoaderContext>): Promise<TResult>
  ruvyxa: {
    kind: "loader"
  }
}

export function loader<TResult>(handler: LoaderHandler<TResult>): Loader<TResult> {
  const callable = async (ctx: Partial<LoaderContext> = {}) => {
    return handler({
      params: ctx.params ?? {},
      request: ctx.request ?? new Request("http://localhost/"),
      cache,
    })
  }

  return Object.assign(callable, {
    ruvyxa: {
      kind: "loader" as const,
    },
  })
}

export interface Schema<TInput> {
  parse(value: unknown): TInput
}

export interface ActionBuilder<TInput = unknown> {
  input<TNextInput>(schema: Schema<TNextInput>): ActionBuilder<TNextInput>
  handler<TResult>(
    handler: (ctx: ActionContext<TInput>) => TResult | Promise<TResult>,
  ): ServerAction<TInput, TResult>
}

export interface ServerAction<TInput, TResult> {
  (input: TInput, ctx?: Partial<ActionContext<TInput>>): Promise<TResult>
  ruvyxa: {
    kind: "action"
  }
}

export const action: ActionBuilder = createActionBuilder()

function createActionBuilder<TInput>(schema?: Schema<TInput>): ActionBuilder<TInput> {
  return {
    input<TNextInput>(nextSchema: Schema<TNextInput>) {
      return createActionBuilder(nextSchema)
    },
    handler<TResult>(handler: (ctx: ActionContext<TInput>) => TResult | Promise<TResult>) {
      const callable = async (rawInput: TInput, ctx: Partial<ActionContext<TInput>> = {}) => {
        const input = schema ? schema.parse(rawInput) : rawInput
        return handler({
          input,
          request: ctx.request ?? new Request("http://localhost/"),
          user: ctx.user,
          invalidate: ctx.invalidate ?? (() => {}),
        })
      }

      return Object.assign(callable, {
        ruvyxa: {
          kind: "action" as const,
        },
      })
    },
  }
}

export interface CacheBuilder {
  ttl(value: string): CacheBuilder
  get<T>(producer: () => T | Promise<T>): Promise<T>
}

// In-memory TTL cache store for server-side data fetching.
const cacheStore = new Map<string, { value: unknown; expiresAt: number }>()

function parseTtl(value: string): number {
  const match = value.match(/^(\d+)\s*(ms|s|m|h|d)$/)
  if (!match) return 60_000 // default 60s
  const amount = parseInt(match[1], 10)
  switch (match[2]) {
    case "ms":
      return amount
    case "s":
      return amount * 1000
    case "m":
      return amount * 60_000
    case "h":
      return amount * 3_600_000
    case "d":
      return amount * 86_400_000
    default:
      return 60_000
  }
}

export function cache(key: string): CacheBuilder {
  let ttlMs = 60_000 // default 60 seconds

  return {
    ttl(value: string) {
      ttlMs = parseTtl(value)
      return this
    },
    async get<T>(producer: () => T | Promise<T>): Promise<T> {
      const now = Date.now()
      const cached = cacheStore.get(key)

      if (cached && cached.expiresAt > now) {
        return cached.value as T
      }

      const value = await producer()
      cacheStore.set(key, { value, expiresAt: now + ttlMs })
      return value
    },
  }
}

/**
 * Invalidate a specific cache key or all keys matching a prefix.
 */
export function invalidateCache(keyOrPrefix?: string): void {
  if (!keyOrPrefix) {
    cacheStore.clear()
    return
  }
  for (const key of cacheStore.keys()) {
    if (key === keyOrPrefix || key.startsWith(keyOrPrefix + ":")) {
      cacheStore.delete(key)
    }
  }
}

export function redirect(location: string, status = 302): Response {
  return new Response(null, {
    status,
    headers: {
      Location: location,
    },
  })
}

export function notFound(): Response {
  return new Response("Not found", { status: 404 })
}

export function json(data: unknown, init?: ResponseInit): Response {
  return Response.json(data, init)
}
