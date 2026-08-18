import { definePlugin } from '@ruvyxa/core/plugin'
import type {
  PresencePluginOptions,
  RealtimePluginOptions,
  RuvyxaPlugin,
} from '@ruvyxa/core/plugin'

export type { PresencePluginOptions, RealtimePluginOptions } from '@ruvyxa/core/plugin'

/** Claim Ruvyxa's versioned native realtime capability. */
export function realtime(options: RealtimePluginOptions = {}): RuvyxaPlugin {
  return definePlugin({
    name: 'ruvyxa:realtime',
    register({ native, build }) {
      native.claim('realtime@1', options)
      build.onComplete(({ manifest }) => {
        assertLongLivedTarget(manifest, 'native WebSocket realtime')
      })
    },
  })
}

/**
 * Claim Ruvyxa's versioned native collaboration capability.
 *
 * Rooms are held in the serving process, so this carries the same deployment
 * constraint as {@link realtime}: a long-lived Node/Bun target. It is stricter
 * in one way — two processes behind a load balancer own two unrelated copies of
 * every room, so a collaborative build must also pin peers of one room to one
 * process.
 */
export function collab(options: PresencePluginOptions = {}): RuvyxaPlugin {
  return definePlugin({
    name: 'ruvyxa:collab',
    register({ native, build }) {
      native.claim('presence@1', options)
      build.onComplete(({ manifest }) => {
        assertLongLivedTarget(manifest, 'native collaboration')
      })
    },
  })
}

/**
 * Both native transports hold per-connection state in the serving process, so
 * both are unusable on a target that can be torn down between requests. The
 * check runs at build time rather than at connect time so the failure lands
 * before deployment instead of after.
 */
function assertLongLivedTarget(manifest: Record<string, unknown>, subject: string): void {
  const target = typeof manifest.target === 'string' ? manifest.target : undefined
  const adapter = adapterName(manifest.adapter)
  const unsupportedAdapter = [
    'aws',
    'cloudflare',
    'firebase',
    'netlify',
    'static',
    'vercel',
  ].includes(adapter ?? '')
  if (target !== 'node' || unsupportedAdapter) {
    const adapterNote = adapter ? ` adapter=${adapter}` : ''
    throw new RealtimeDeploymentError(
      `${subject} requires a long-lived Node/Bun build; received target=${target ?? 'unknown'}${adapterNote}`,
    )
  }
}

function adapterName(value: unknown): string | undefined {
  if (typeof value === 'string') return value.toLowerCase()
  if (value && typeof value === 'object' && 'name' in value) {
    const name = (value as { name?: unknown }).name
    if (typeof name === 'string') return name.toLowerCase()
  }
  return undefined
}

export class RealtimeDeploymentError extends Error {
  readonly code = 'RUV3201'

  constructor(message: string) {
    super(`RUV3201 ${message}`)
    this.name = 'RealtimeDeploymentError'
  }
}
