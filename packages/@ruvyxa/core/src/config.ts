import type { RuvyxaConfig } from "./types.js"

export type {
  BuiltinMiddlewareConfig,
  CorsConfig,
  LayerConfig,
  MiddlewareConfig,
  MiddlewarePluginConfig,
  PluginPermissions,
  PluginContext,
  RateLimitConfig,
  RuvyxaConfig,
  RuvyxaPlugin,
  TransformResult,
} from "./types.js"

export function defineConfig<TConfig extends RuvyxaConfig>(config: TConfig): TConfig {
  return config
}
