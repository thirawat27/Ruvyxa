import type { RuvyxaConfig } from './types.js'

export type {
  BuiltinMiddlewareConfig,
  CachedStaticParams,
  ContentConfig,
  ContentEngineConfig,
  CorsConfig,
  EsTarget,
  GetStaticParams,
  ImageConfig,
  I18nConfig,
  MarkdownConfig,
  MarkdownPlugin,
  MarkdownPluginEntry,
  MarkdownPluginList,
  MarkdownPluginPreset,
  MiddlewareConfig,
  OnDemandImageConfig,
  PageProps,
  RateLimitConfig,
  RenderConfig,
  RenderStrategy,
  RouteParamValue,
  RouteParams,
  RuvyxaConfig,
  SiteConfig,
  StaticParamsContext,
  StaticParamSegment,
  StaticParamsCacheDuration,
  StaticParamsResult,
  StaticParamsValues,
  TransformResult,
} from './types.js'

/** Define the typed contents of `ruvyxa.config.ts`. */
export function config<TConfig extends RuvyxaConfig>(config: TConfig): TConfig {
  return config
}
