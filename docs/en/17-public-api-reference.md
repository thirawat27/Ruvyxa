# Public API reference

> **Tutorial goal:** choose the smallest public API for the lesson you are implementing. **Start
> from:** the practical route and data examples in chapters 4–9. **Checkpoint:** import from a
> documented public entry point rather than an internal source path.

This reference lists stable exported surfaces found in package entry points. It intentionally
separates implementation details in Rust/runtime files from the APIs applications import.

## `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`

| Export                                          | Signature / purpose                                                                        |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------ |
| `config`                                        | `<T extends RuvyxaConfig>(config: T) => T`; typed config identity helper.                  |
| `loader`                                        | `(handler: LoaderHandler<T>) => Loader<T>`; handler gets `params`, `request`, `cache`.     |
| `action`                                        | Builder: `.input(schema)`, `.realtime(channels?)`, `.handler(fn)`.                         |
| `cache`                                         | `(key) => CacheBuilder`; `.ttl`, `.swr`, `.tags(...keys)`, `.scope(...)`, and `.get(...)`. |
| `invalidateCache`, `cacheStats`                 | Remove exact/prefix/all cache entries; report `{ size, maxEntries }`.                      |
| `FlightContext`, `FlightHandler`, `FlightValue` | Types for a public `flight` route export and the payload it returns.                       |
| `json`, `redirect`, `notFound`                  | Response helpers; redirect only permits 3xx statuses.                                      |
| `cookies`, `headers`, `draftMode`               | Read the request being served. Calling any of them keeps the render out of shared caches.  |
| `revalidatePath`                                | `(path: string) => void`; queue one concrete URL for re-render on its next request.        |
| `definePlugin`, `withResponseHeader`            | Plugin definition and response-header helper.                                              |
| `standaloneServerSource`                        | Source generator for the standalone server artifact.                                       |

Types include `RuvyxaConfig`, `PageProps`, `GetStaticParams`, `RenderStrategy`, `Adapter`,
`MiddlewareConfig`, `ImageConfig`, `I18nConfig`, `SiteConfig`, and plugin contracts. Use imports
from `ruvyxa` for public primitives and `ruvyxa/config` or `ruvyxa/plugin` for explicit intent.

## `@ruvyxa/react`

| Export family         | Main names                                                                                                                             |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Navigation            | `Link`, `RouteContext`, `useRouter`, `usePathname`, `useParams`, `useSearchParams`, `useSelectedRoute`, `useRouteContext`, `useFlight` |
| Rendering errors      | `RuvyxaErrorBoundary`, `notFound`, `isNotFoundError`, `RouteErrorProps`                                                                |
| Metadata/content      | `Seo`, `Meta`, `MetaFactory`, `Answer`                                                                                                 |
| Browser/runtime       | `hydrate`, `reportHydrationError`, `useRuvyxaLoader`                                                                                   |
| Assets                | `Image`, `Picture`, `Script`, `DEFAULT_DEVICE_WIDTHS`                                                                                  |
| Typed routes          | `route`, `RouteHref`, `RoutePattern`, `KnownRoute`, `RuvyxaRouteRegistry`                                                              |
| Low-level integration | `getRouterInstance`, `resetInjectedScripts`, `NOT_FOUND_PROPERTY`                                                                      |

`useRuvyxaLoader<T>(loader, { enabled?, deps? })` returns `{ data, loading, error, refetch }`.
`hydrate({ root?, onError? })` dispatches the hydration event and installs optional reporting.
`notFound()` from this package always throws and therefore returns `never`. `<Script strategy>` is
`beforeInteractive`, `afterInteractive` (default), or `lazyOnload`. `RouteHref` is `string` unless
`typedRoutes` is enabled and the generated declaration file is in the tsconfig `include`;
`route(href)` asserts a runtime string into it.

`useFlight<T>()` reads the public payload from the current soft navigation. It is `undefined` when
the matched route has no `flight` export, or when the first server-rendered document did not include
an inline payload. `getRouterInstance`, `resetInjectedScripts`, and `NOT_FOUND_PROPERTY` are
low-level integration and test seams; application code should normally use the hooks and components
above.

## `@ruvyxa/core/route-match`

The route matcher every JavaScript host shares — the browser router, the serverless handler, and the
standalone server all resolve a URL through this one module, so a link click and a reload of the
same address cannot disagree.

| Export                                            | Purpose                                                                      |
| ------------------------------------------------- | ---------------------------------------------------------------------------- |
| `createRouteMatcher(routes)`                      | Compile a route table once; returns `(pathname) => RouteMatch \| null`.      |
| `canonicalRoutePath(pathname)`                    | Decode a path once into canonical segments, or `null` if it must be refused. |
| `compilePattern`, `routeSpecificity`              | Pattern compilation and static-before-dynamic ordering.                      |
| `compareSpecificity`, `normalizeMatchPath`        | Ordering and slash normalization primitives.                                 |
| `bindPatternParams(pattern, matched)`             | Bind a compiled pattern's captures to named parameters.                      |
| `RouteParams`, `RouteMatch`, `RouteManifestEntry` | Matching types.                                                              |

Applications normally need only `useParams()` from `@ruvyxa/react`. These are for code that has to
resolve routes outside the React tree, such as a custom server or adapter.

> **Changed in 1.0.28.** These names were previously re-exported from `@ruvyxa/react` and are no
> longer. `@ruvyxa/react` still exports the `RouteParams` type; import the rest from
> `@ruvyxa/core/route-match`.

## Other public packages

| Package             | Exported integration                                                                          |
| ------------------- | --------------------------------------------------------------------------------------------- |
| `@ruvyxa/auth`      | `createAuth`, providers, stores, client/plugin entry points, auth types/errors.               |
| `@ruvyxa/database`  | `createDatabase`, operation/types, `prismaAdapter`, `dynamoAdapter`, `defineDatabaseAdapter`. |
| `@ruvyxa/realtime`  | Plugin entry point; client exposes `createRealtimeClient`.                                    |
| `@ruvyxa/testing`   | `mockLoader`, `mockAction`, `mockCache`.                                                      |
| `@ruvyxa/adapter-*` | Typed build adapter packages.                                                                 |

For option details and defaults, use [Configuration](07-configuration.md) and the exported
TypeScript declarations in the installed package. Public API names shown here are source-verified;
runtime names beginning with `RUVYXA_` and double underscores are not application API.

**Previous:** [Troubleshooting and upgrade compatibility](16-troubleshooting-upgrades.md) ·
**Next:** [Documentation scope and sources](18-documentation-scope-and-sources.md)
