import { definePlugin } from 'ruvyxa/plugin'

/**
 * A Ruvyxa plugin.
 *
 * `headers` is the concise form: every response gains the header, and Ruvyxa
 * registers the hook for you. Add `http`, `build`, `dev`, `diagnostics`, or
 * `native` sections when the plugin needs them, and reach for `register()` only
 * when one section cannot express the composition.
 */
export default definePlugin({
  name: '__PLUGIN_NAME__',
  headers: { 'x-__PLUGIN_NAME__': 'active' },
})
