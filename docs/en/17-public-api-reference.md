# Public API reference

> **Tutorial goal:** choose the smallest public API for the lesson you are implementing. **Start
> from:** the practical route and data examples in chapters 4–9. **Checkpoint:** import from a
> documented public entry point rather than an internal source path.

This reference lists stable exported surfaces found in package entry points. It intentionally
separates implementation details in Rust/runtime files from the APIs applications import.

## `ruvyxa`, `ruvyxa/server`, and `ruvyxa/config`

The **From** column is the part worth reading first, because the entry points are not
interchangeable. `ruvyxa` re-exports the primitives any module may use. The request-scoped calls —
`cookies`, `headers`, `params`, `draftMode` — are only on `ruvyxa/server`, and they read a store the
runtime installs around a render or handler. Calling one at module scope, or from browser code,
throws `… was called outside a request` rather than returning an empty value, so the import path is
also the clearest statement of where the code is meant to run.

| Export                                          | From                        | Signature / purpose                                                                                                                                                                                                      |
| ----------------------------------------------- | --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `config`                                        | `ruvyxa/config` or `ruvyxa` | `<T extends RuvyxaConfig>(config: T) => T`; typed config identity helper.                                                                                                                                                |
| `loader`                                        | both                        | `(handler: LoaderHandler<T>) => Loader<T>`; handler gets `params`, `request`, `cache`.                                                                                                                                   |
| `action`                                        | both                        | Builder: `.input(schema)`, `.realtime(channels?)`, `.handler(fn)`.                                                                                                                                                       |
| `cache`                                         | both                        | `(key) => CacheBuilder`; `.ttl`, `.swr`, `.tags(...keys)`, `.scope(...)`, and `.get(...)`.                                                                                                                               |
| `invalidateCache`, `cacheStats`                 | both                        | Remove exact/prefix/all cache entries; report `{ size, maxEntries }`.                                                                                                                                                    |
| `pruneCache`                                    | both                        | `() => number`; drop every fully expired entry and report how many went. What the module's own sweep runs every 60s; an entry past its stale window can no longer be served, so this frees memory and changes no answer. |
| `revalidateTag`                                 | `ruvyxa/server`             | `(tag: string) => void`; drop every cache entry carrying that exact tag.                                                                                                                                                 |
| `json`, `redirect`, `status`                    | both                        | Response helpers. `redirect` permits only 3xx; `status(code, message?)` builds any 200–599 response and refuses a body on 204/205/304. The throwing `notFound()` is `@ruvyxa/react`'s.                                   |
| `cookies`, `headers`, `draftMode`               | `ruvyxa/server`             | Read the request being served. Calling any of them keeps the render out of shared caches.                                                                                                                                |
| `params`                                        | `ruvyxa/server`             | Route parameters for the page being rendered, readable below the component that got props.                                                                                                                               |
| `revalidatePath`                                | `ruvyxa/server`             | `(path: string) => void`; queue one concrete URL for re-render on its next request.                                                                                                                                      |
| `FlightContext`, `FlightHandler`, `FlightValue` | `ruvyxa/server`             | Types for a public `flight` route export and the payload it returns.                                                                                                                                                     |
| `definePlugin`, `withResponseHeader`            | `ruvyxa/plugin` or `ruvyxa` | Plugin definition and response-header helper.                                                                                                                                                                            |
| `standaloneServerSource`                        | `ruvyxa`                    | Source generator for the standalone server artifact.                                                                                                                                                                     |

"Both" means the name is re-exported from `ruvyxa` as well as `ruvyxa/server`; prefer
`ruvyxa/server` in server-only modules so the import itself states the boundary.

Adapter authors also get build helpers from `ruvyxa`, in four groups.

| Group                     | Names                                                                                                                                                                                                                                                  |
| ------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Build context and out dir | `validateBuildContext`, `clientBuildOutput`, `runtimeBuildPolicy`, `projectRelativeOutDir`, `assertSafeOutDirForCommand`                                                                                                                               |
| Assets and headers        | `staticAssetGlobs`, `publicAssetGlobs`, `staticAssetPattern`, `headersFileContents`, `DEFAULT_SECURITY_HEADERS`, `IMMUTABLE_CACHE_CONTROL`, `PUBLIC_ASSET_CACHE_CONTROL`, `STATIC_ASSET_EXTENSIONS`, `CLIENT_BUNDLE_PREFIX`, `DEFAULT_IMAGE_MAX_WIDTH` |
| Deploy manifest           | `parseDeployManifest`, `deployHeaderRules`, `documentCacheControl`, `routeServeMode`, `nonPublishableStrategies`, `DEPLOY_MANIFEST_KEY`, `DEPLOY_MANIFEST_VERSION`, `DOCUMENT_CACHE_CONTROL`                                                           |
| Emitted document stores   | `platformDocumentStoreSource`, `documentCacheOptionsSource`, `isrTemporaryCacheSource`, `isrTemporaryCacheDirSource`                                                                                                                                   |

The last row is source an adapter emits into its generated handler rather than something it calls at
build time: `platformDocumentStoreSource` returns the ISR/PPR document store a platform's wrapper
installs, so eleven adapters do not carry eleven copies of it. All of them are used in context in
the [Platform adapter guide](20-platform-adapter-guide.md).

Types include `RuvyxaConfig`, `PageProps`, `GetStaticParams`, `RenderStrategy`, `Adapter`,
`AdapterInspection`, `MiddlewareConfig`, `ImageConfig`, `I18nConfig`, `SiteConfig`, the site
subtypes `SiteSitemapConfig`, `SiteSitemapEntry`, `SiteSitemapEntryDefaults`, `SiteSitemapVideo`,
`SiteRobotsConfig`, and `SiteRobotsRule`, the content subtypes `ContentConfig` and
`ContentEngineConfig`, the deploy-manifest types `DeployManifest`, `DeployRoute`, and
`DeployServeMode`, and plugin contracts. Use imports from `ruvyxa` for public primitives and
`ruvyxa/config` or `ruvyxa/plugin` for explicit intent.

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
`hydrate({ root?, onError? })` dispatches the hydration event and installs the reporter the
generated entry hands React's `onRecoverableError`, `onCaughtError`, and `onUncaughtError` to;
reports raised before it is installed are queued and delivered on install, and `context.kind` names
the callback. `notFound()` from this package always throws and therefore returns `never`.
`<Script strategy>` is `beforeInteractive`, `afterInteractive` (default), or `lazyOnload`.
`RouteHref` is `string` unless `typedRoutes` is enabled and the generated declaration file is in the
tsconfig `include`; `route(href)` asserts a runtime string into it.

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

> `@ruvyxa/react` exports the `RouteParams` type. Every other name in this table comes from
> `@ruvyxa/core/route-match` and only from there.

## Other public packages

| Package             | Exported integration                                                                                                                                       |
| ------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `@ruvyxa/auth`      | `createAuth`, providers, stores, client/plugin entry points, auth types/errors, and `forwardedClientIp(request)` for rate limiting behind a trusted proxy. |
| `@ruvyxa/database`  | `createDatabase`, operation/types, `prismaAdapter`, `dynamoAdapter`, `defineDatabaseAdapter`.                                                              |
| `@ruvyxa/realtime`  | Plugin entry point; client exposes `createRealtimeClient`.                                                                                                 |
| `@ruvyxa/testing`   | `mockLoader`, `mockAction`, `mockCache`.                                                                                                                   |
| `@ruvyxa/adapter-*` | Typed build adapter packages.                                                                                                                              |

For option details and defaults, use [Configuration](07-configuration.md) and the exported
TypeScript declarations in the installed package. Public API names shown here are source-verified;
runtime names beginning with `RUVYXA_` and double underscores are not application API.

**Previous:** [Troubleshooting and upgrade compatibility](16-troubleshooting-upgrades.md) ·
**Next:** [Documentation scope and sources](18-documentation-scope-and-sources.md)
