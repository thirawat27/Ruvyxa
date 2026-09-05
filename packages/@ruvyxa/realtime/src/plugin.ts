import { definePlugin } from '@ruvyxa/core/plugin'
import type {
  PresencePluginOptions,
  RealtimePluginOptions,
  RuvyxaPlugin,
} from '@ruvyxa/core/plugin'

export type { PresencePluginOptions, RealtimePluginOptions } from '@ruvyxa/core/plugin'

/**
 * Claim Ruvyxa's versioned native realtime capability.
 *
 * The plugin claims; it does not decide where the claim can be honoured. The
 * socket is served by the Axum host — `ruvyxa dev`, `ruvyxa start`,
 * `ruvyxa preview` — and by no build artifact at all: not a serverless
 * function, and not the standalone server the node, bun, deno, railway, and
 * render adapters emit, which speaks plain HTTP with no upgrade path. That rule
 * is owned by `adapter-runner.mjs`, which reports `RUV2205` naming the
 * capability and its path for every adapter build. A gate here used to
 * re-derive it from process lifetime — refusing six adapters with `RUV3201`
 * and passing the other five — and so failed a vercel build outright while a
 * railway build shipped `/__ruvyxa/realtime` as a 404.
 */
export function realtime(options: RealtimePluginOptions = {}): RuvyxaPlugin {
  return definePlugin({
    name: 'ruvyxa:realtime',
    register({ native }) {
      native.claim('realtime@1', options)
    },
  })
}

/**
 * Claim Ruvyxa's versioned native collaboration capability.
 *
 * Rooms are held in the serving process, so this carries the same deployment
 * shape as {@link realtime}: served under `ruvyxa start`, reported as `RUV2205`
 * by every adapter build. It is stricter in one way — two processes behind a
 * load balancer own two unrelated copies of every room, so a collaborative
 * deployment must also pin peers of one room to one process.
 */
export function collab(options: PresencePluginOptions = {}): RuvyxaPlugin {
  return definePlugin({
    name: 'ruvyxa:collab',
    register({ native }) {
      native.claim('presence@1', options)
    },
  })
}
