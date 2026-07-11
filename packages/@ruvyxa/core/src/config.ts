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
  RenderingConfig,
  RenderStrategy,
  RuvyxaConfig,
  RuvyxaPlugin,
  StaticParamsContext,
  TransformResult,
} from './types.js'

export function defineConfig<TConfig extends RuvyxaConfig>(config: TConfig): TConfig {
  return config
}
