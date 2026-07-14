import type { RuvyxaConfig } from './types.js'

export type {
  BuiltinMiddlewareConfig,
  CorsConfig,
  GetStaticParams,
  ImageConfig,
  LayerConfig,
  MiddlewareConfig,
  MiddlewarePluginConfig,
  PageProps,
  PluginPermissions,
  PluginContext,
  RateLimitConfig,
  RenderConfig,
  RenderStrategy,
  RouteParamValue,
  RouteParams,
  RuvyxaConfig,
  RuvyxaPlugin,
  StaticParamsContext,
  TransformResult,
} from './types.js'

/** Define the typed contents of `ruvyxa.config.ts`. */
export function config<TConfig extends RuvyxaConfig>(config: TConfig): TConfig {
  return config
}
