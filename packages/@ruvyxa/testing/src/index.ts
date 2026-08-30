import {
  assertCacheKey,
  assertCacheSerializable,
  assertSharedCachePrivacy,
  normalizeCacheTags,
  parseTtl,
} from '@ruvyxa/core/server'
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

export interface MockCacheOptions {
  /**
   * Whether the double stands in for a cache used inside a request.
   *
   * The real builder throws for `scope('request')` outside one, so a suite
   * exercising a loader that must not be request-scoped sets this to `false` and
   * gets the production failure instead of a silent pass.
   */
  requestContext?: boolean
}

/**
 * A `cache()` double that refuses everything the real builder refuses.
 *
 * Every method used to be a plain assignment, so
 * `cache('posts').ttl('5 minutes').get(async () => ({ published: new Date() }))`
 * passed its test and threw twice in production — once on the duration, once on
 * the value. The validators are imported from `@ruvyxa/core/server` rather than
 * re-stated here: a helper whose whole job is parity cannot own a second copy of
 * the rules it is checking.
 */
export function mockCache(
  seed: Readonly<Record<string, unknown>> = {},
  options: MockCacheOptions = {},
): MockCache {
  const values = new Map(Object.entries(seed))
  const calls: MockCacheCall[] = []
  const requestContext = options.requestContext ?? true
  const callable = (key: string): CacheBuilder => {
    assertCacheKey(key)
    let ttl: string | undefined
    let swr: string | undefined
    let tags: readonly string[] = []
    let scope: 'deployment' | 'request' = 'deployment'
    const builder: CacheBuilder = {
      ttl(value) {
        parseTtl(value)
        ttl = value
        return builder
      },
      swr(value) {
        parseTtl(value)
        swr = value
        return builder
      },
      tags(...values) {
        // Validated, de-duplicated, and code-unit ordered by @ruvyxa/core, so
        // the recorded call carries the same tag identity a real entry would.
        tags = normalizeCacheTags(values)
        return builder
      },
      scope(value) {
        if (value !== 'deployment' && value !== 'request') {
          throw new TypeError('cache().scope() must be "deployment" or "request"')
        }
        scope = value
        return builder
      },
      async get<T>(producer: () => T | Promise<T>) {
        if (scope === 'request' && !requestContext) {
          throw new Error('request-scoped cache used outside a request')
        }
        const hit = scope === 'deployment' && values.has(key)
        calls.push({ key, ttl, swr, tags, scope, hit })
        if (hit) return values.get(key) as T
        const value = await producer()
        // Order matters: the real builder awaits the producer before asserting
        // on the value, and checks privacy only for the shared scope. Checking
        // earlier, or checking privacy on a request-scoped read, would diverge
        // in the other direction.
        if (scope === 'deployment') assertSharedCachePrivacy()
        assertCacheSerializable(value)
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
