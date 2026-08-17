import type {
  ActionContext,
  CacheBuilder,
  LoaderContext,
  ServerAction,
  Loader,
} from '@ruvyxa/core/server'

export interface MockControl<TCall> {
  readonly calls: readonly TCall[]
  reset(): void
}

export type LoaderMock<TResult> = Loader<TResult> & MockControl<LoaderContext>

export function mockLoader<TResult>(
  result: TResult | ((context: LoaderContext) => TResult | Promise<TResult>),
  defaults: Partial<LoaderContext> = {},
): LoaderMock<TResult> {
  const calls: LoaderContext[] = []
  const callable = async (context: Partial<LoaderContext> = {}) => {
    const normalized: LoaderContext = {
      params: context.params ?? defaults.params ?? {},
      request: context.request ?? defaults.request ?? new Request('http://localhost/__ruvyxa/test'),
      cache: context.cache ?? defaults.cache ?? mockCache(),
    }
    calls.push(normalized)
    return typeof result === 'function'
      ? (result as (context: LoaderContext) => TResult | Promise<TResult>)(normalized)
      : result
  }
  return Object.assign(callable, {
    ruvyxa: { kind: 'loader' as const },
    get calls() {
      return calls as readonly LoaderContext[]
    },
    reset() {
      calls.length = 0
    },
  })
}

export interface ActionMockCall<TInput> {
  input: TInput
  context: ActionContext<TInput>
}

export type ActionMock<TInput, TResult> = ServerAction<TInput, TResult> &
  MockControl<ActionMockCall<TInput>> & {
    readonly invalidations: readonly string[]
  }

export function mockAction<TInput, TResult>(
  result: TResult | ((context: ActionContext<TInput>) => TResult | Promise<TResult>),
  defaults: Partial<ActionContext<TInput>> = {},
): ActionMock<TInput, TResult> {
  const calls: ActionMockCall<TInput>[] = []
  const invalidations: string[] = []
  const callable = async (input: TInput, context: Partial<ActionContext<TInput>> = {}) => {
    const callerInvalidate = context.invalidate ?? defaults.invalidate
    const normalized: ActionContext<TInput> = {
      input,
      request: context.request ?? defaults.request ?? new Request('http://localhost/__ruvyxa/test'),
      user: context.user ?? defaults.user,
      invalidate(key) {
        invalidations.push(key)
        callerInvalidate?.(key)
      },
    }
    calls.push({ input, context: normalized })
    return typeof result === 'function'
      ? (result as (context: ActionContext<TInput>) => TResult | Promise<TResult>)(normalized)
      : result
  }
  return Object.assign(callable, {
    ruvyxa: { kind: 'action' as const },
    get calls() {
      return calls as readonly ActionMockCall<TInput>[]
    },
    get invalidations() {
      return invalidations as readonly string[]
    },
    reset() {
      calls.length = 0
      invalidations.length = 0
    },
  })
}

export interface MockCacheCall {
  key: string
  ttl?: string
  swr?: string
  tags: readonly string[]
  scope: 'deployment' | 'request'
  hit: boolean
}

export interface MockCache extends MockControl<MockCacheCall> {
  (key: string): CacheBuilder
  set(key: string, value: unknown): void
  delete(key: string): boolean
  clear(): void
}

export function mockCache(seed: Readonly<Record<string, unknown>> = {}): MockCache {
  const values = new Map(Object.entries(seed))
  const calls: MockCacheCall[] = []
  const callable = (key: string): CacheBuilder => {
    let ttl: string | undefined
    let swr: string | undefined
    let tags: readonly string[] = []
    let scope: 'deployment' | 'request' = 'deployment'
    const builder: CacheBuilder = {
      ttl(value) {
        ttl = value
        return builder
      },
      swr(value) {
        swr = value
        return builder
      },
      tags(...values) {
        tags = [...new Set(values)].sort((a, b) => a.localeCompare(b))
        return builder
      },
      scope(value) {
        scope = value
        return builder
      },
      async get<T>(producer: () => T | Promise<T>) {
        const hit = scope === 'deployment' && values.has(key)
        calls.push({ key, ttl, swr, tags, scope, hit })
        if (hit) return values.get(key) as T
        const value = await producer()
        if (scope === 'deployment') values.set(key, value)
        return value
      },
    }
    return builder
  }
  return Object.assign(callable, {
    get calls() {
      return calls as readonly MockCacheCall[]
    },
    set(key: string, value: unknown) {
      values.set(key, value)
    },
    delete(key: string) {
      return values.delete(key)
    },
    clear() {
      values.clear()
    },
    reset() {
      values.clear()
      for (const [key, value] of Object.entries(seed)) values.set(key, value)
      calls.length = 0
    },
  })
}
