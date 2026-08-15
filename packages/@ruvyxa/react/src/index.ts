export { RuvyxaErrorBoundary } from './error-boundary.js'
export type { ErrorBoundaryProps, ErrorFallbackProps } from './error-boundary.js'
export { useRuvyxaLoader } from './use-loader.js'
export type { UseLoaderOptions, UseLoaderResult } from './use-loader.js'
export { hydrate, reportHydrationError } from './hydration.js'
export type { HydrationOptions, HydrationErrorHandler } from './hydration.js'
export { DEFAULT_DEVICE_WIDTHS, Image, Picture } from './image.js'
export type {
  ImageLoader,
  ImageLoaderProps,
  ImageProps,
  PictureProps,
  PictureSource,
} from './image.js'
export { Script, resetInjectedScripts } from './script.js'
export type { ScriptProps, ScriptStrategy } from './script.js'
export { Seo } from './seo.js'
export type { SeoArticle, SeoAuthor, SeoBreadcrumb, SeoProps } from './seo.js'
export type { Meta, MetaAlternate, MetaContext, MetaExport, MetaFactory } from './meta.js'
export { Answer } from './answer.js'
export type { AnswerProps, AnswerSource } from './answer.js'
export { Link } from './link.js'
export type { LinkPrefetch, LinkProps } from './link.js'
export {
  RouteContext,
  useParams,
  usePathname,
  useFlight,
  useRouteContext,
  useRouter,
  useSearchParams,
  useSelectedRoute,
} from './route-context.js'
export { route } from './route-types.js'
export type {
  ExternalHref,
  KnownRoute,
  RouteFromPattern,
  RouteHref,
  RoutePattern,
  RuvyxaRouteRegistry,
} from './route-types.js'
export { getRouterInstance } from './router.js'
export type { NavigateOptions, RouteContextValue, RouterInstance, RuvyxaRouter } from './router.js'
export { isNotFoundError, notFound, NOT_FOUND_PROPERTY } from './not-found.js'
export type { NotFoundError } from './not-found.js'
export type { RouteErrorProps } from './special-files.js'
// `RouteParams` is what `useParams()` returns, so it stays part of this
// package's surface. The pattern compiler, specificity ordering, and matcher
// factory behind it are engine internals shared with the server and serverless
// hosts: their home is `@ruvyxa/core/route-match`, and re-exporting them here
// invited the duplicate ports this package used to carry.
export type { RouteParams } from '@ruvyxa/core/route-match'
