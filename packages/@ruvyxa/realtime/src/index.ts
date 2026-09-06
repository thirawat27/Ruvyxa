/**
 * The realtime and collaboration transports are turned on in `ruvyxa.config.ts`
 * (`realtime: true`, `collab: true`, or their option objects) and served by the
 * Axum host. This package ships the clients: `@ruvyxa/realtime/client`,
 * `@ruvyxa/realtime/collab`, and `@ruvyxa/realtime/react`.
 */
export type { CollabConfig, RealtimeConfig } from '@ruvyxa/core'
