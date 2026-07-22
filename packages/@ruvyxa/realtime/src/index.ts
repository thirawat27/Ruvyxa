import { definePlugin } from '@ruvyxa/core/config'
import type { RealtimePluginOptions, RuvyxaPlugin } from '@ruvyxa/core/config'

export type { RealtimePluginOptions } from '@ruvyxa/core/config'

/** Enable Ruvyxa's native WebSocket transport on self-hosted Node/Bun deployments. */
export function realtime(options: RealtimePluginOptions = {}): RuvyxaPlugin {
  return definePlugin({
    name: 'ruvyxa:realtime',
    setup({ enableRealtime, onBuildComplete }) {
      enableRealtime(options)
      onBuildComplete(({ manifest }) => {
        const target = typeof manifest.target === 'string' ? manifest.target : undefined
        const adapter = adapterName(manifest.adapter)
        const unsupportedAdapter = ['cloudflare', 'netlify', 'static', 'vercel'].includes(
          adapter ?? '',
        )
        if (target !== 'node' || unsupportedAdapter) {
          throw new RealtimeDeploymentError(
            `native WebSocket realtime requires a self-hosted Node/Bun build; received target=${target ?? 'unknown'}${adapter ? ` adapter=${adapter}` : ''}`,
          )
        }
      })
    },
  })
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
