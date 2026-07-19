import type { RuvyxaConfig, RuvyxaPlugin } from './types.js'

export type {
  BuiltinMiddlewareConfig,
  CachedStaticParams,
  CorsConfig,
  GetStaticParams,
  ImageConfig,
  MiddlewareConfig,
  PageProps,
  PluginBuildCompleteHook,
  PluginBuildContext,
  PluginEnvironment,
  PluginMiddleware,
  PluginMiddlewareContext,
  PluginRequestMiddleware,
  PluginRequestResult,
  PluginResolveIdHook,
  PluginResponseMiddleware,
  PluginSetupContext,
  PluginTransformContext,
  PluginTransformHook,
  RateLimitConfig,
  RenderConfig,
  RenderStrategy,
  RouteParamValue,
  RouteParams,
  RuvyxaConfig,
  RuvyxaPlugin,
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

/** Define a named plugin for use in `ruvyxa.config.ts`. */
export function definePlugin(plugin: RuvyxaPlugin): RuvyxaPlugin {
  if (!plugin || typeof plugin !== 'object') {
    throw new TypeError('Ruvyxa plugin must be an object.')
  }
  if (typeof plugin.name !== 'string' || plugin.name.trim() === '') {
    throw new TypeError('Ruvyxa plugin must have a non-empty name.')
  }
  if (typeof plugin.setup !== 'function') {
    throw new TypeError(`Ruvyxa plugin "${plugin.name}" must provide setup(context).`)
  }
  return Object.freeze({ ...plugin, name: plugin.name.trim() })
}
