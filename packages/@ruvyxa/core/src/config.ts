import type {
  RuvyxaConfig,
  RuvyxaPlugin,
  RuvyxaPluginFactory,
  RuvyxaPluginHooks,
  RuvyxaPluginTransformHook,
} from "./types.js"

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
  RuvyxaPluginFactory,
  RuvyxaPluginFactoryResult,
  RuvyxaPluginHooks,
  RuvyxaPluginInput,
  RuvyxaPluginTransformHook,
  PluginSetupContext,
  PluginTransformResult,
  TransformResult,
} from "./types.js"

export function defineConfig<TConfig extends RuvyxaConfig>(config: TConfig): TConfig {
  return config
}

export function definePlugin<TPlugin extends RuvyxaPlugin | RuvyxaPluginFactory>(plugin: TPlugin): TPlugin
export function definePlugin(name: string, transform: RuvyxaPluginTransformHook): RuvyxaPlugin
export function definePlugin(name: string, hooks: RuvyxaPluginHooks): RuvyxaPlugin
export function definePlugin(
  pluginOrName: string | RuvyxaPlugin | RuvyxaPluginFactory,
  hooksOrTransform?: RuvyxaPluginHooks | RuvyxaPluginTransformHook,
): RuvyxaPlugin | RuvyxaPluginFactory {
  if (typeof pluginOrName !== "string") {
    return pluginOrName
  }

  if (typeof hooksOrTransform === "function") {
    return {
      name: pluginOrName,
      transform: hooksOrTransform,
    }
  }

  return {
    name: pluginOrName,
    ...(hooksOrTransform ?? {}),
  }
}

export const plugin = definePlugin
