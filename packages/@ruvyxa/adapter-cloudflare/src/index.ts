import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for the Cloudflare Workers deployment adapter.
 */
export interface CloudflareAdapterOptions {
  /** Custom worker entry point path. Defaults to `${outDir}/server/app`. */
  workerEntry?: string
  /**
   * Also emit a `wrangler.jsonc` at the project root pointing at the
   * generated Worker script and static assets, so `wrangler deploy` works
   * right after `ruvyxa build` with no dashboard configuration. An existing
   * project `wrangler.jsonc` is never overwritten.
   * @default true
   */
  projectConfig?: boolean
  /**
   * Cloudflare Workers compatibility date. This determines which runtime
   * APIs are available. Defaults to the current date at build time.
   */
  compatibilityDate?: string
}

/**
 * Worker fetch handler source code.
 *
 * This is the platform-specific entry that wraps the generic Ruvyxa serverless
 * handler into a Cloudflare Workers `fetch` event handler. It reads the route
 * manifest and delegates to the serverless handler for SSR/API/ISR/PPR.
 *
 * Static assets (client bundles, pre-rendered pages for SSG/CSR) are served
 * by Cloudflare's `assets` binding; the Worker only handles dynamic routes.
 */
function workerHandlerSource(): string {
  return `import { createHandler } from './serverless-handler.mjs';
import { loadRouteModule } from './route-modules.mjs';
import manifest from './manifest.json';

const handler = createHandler({
  routes: manifest.routes,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  readPrerendered: (pathname) => {
    // In Workers, pre-rendered pages are served as static assets.
    // ISR revalidation requires KV or Durable Objects (not yet supported).
    return null;
  },
  supportedStrategies: ['ssr', 'ssg', 'csr', 'api'],
});

export default {
  async fetch(request, env, ctx) {
    return handler(request);
  },
};
`
}

/**
 * Create a Cloudflare Workers deployment adapter for Ruvyxa.
 *
 * Produces a Worker fetch handler and static assets for deployment via
 * `wrangler`. Supports SSR, API routes, SSG, and CSR. ISR and PPR are
 * rejected with RUV2210 because they require persistent storage (KV/DO)
 * which is not yet integrated.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { cloudflareAdapter } from "@ruvyxa/adapter-cloudflare"
 *
 * export default config({
 *   adapter: cloudflareAdapter()
 * })
 * ```
 */
export function cloudflareAdapter(options: CloudflareAdapterOptions = {}): Adapter {
  if (options.workerEntry !== undefined && typeof options.workerEntry !== 'string') {
    throw new Error(
      `[RUV2001] cloudflareAdapter: "workerEntry" must be a string, got ${typeof options.workerEntry}`,
    )
  }

  if (options.workerEntry !== undefined && options.workerEntry.trim() === '') {
    throw new Error(`[RUV2001] cloudflareAdapter: "workerEntry" must not be an empty string`)
  }

  return {
    name: 'cloudflare',
    target: 'edge',
    supports: ['ssr', 'ssg', 'csr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'cloudflareAdapter')

      const compatDate = options.compatibilityDate ?? new Date().toISOString().slice(0, 10)

      const wranglerConfig = JSON.stringify(
        {
          name: 'ruvyxa-app',
          main: './worker/index.mjs',
          compatibility_date: compatDate,
          assets: { directory: './assets' },
        },
        null,
        2,
      )

      const projectWranglerConfig = JSON.stringify(
        {
          name: 'ruvyxa-app',
          main: `${ctx.outDir}/deploy/cloudflare/worker/index.mjs`,
          compatibility_date: compatDate,
          assets: { directory: `${ctx.outDir}/deploy/cloudflare/assets` },
        },
        null,
        2,
      )

      return {
        name: 'cloudflare',
        target: 'edge',
        platform: 'cloudflare',
        entry: options.workerEntry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        configFiles: ['wrangler.jsonc'],
        artifacts: [
          // Static assets served by Cloudflare's asset binding
          { kind: 'static-site', path: 'deploy/cloudflare/assets' },
          // Worker function bundle (SSR/API handler)
          {
            kind: 'function',
            path: 'deploy/cloudflare/worker',
            handlerSource: workerHandlerSource(),
          },
          // Wrangler config pointing at the Worker + assets
          {
            kind: 'file',
            path: 'deploy/cloudflare/wrangler.jsonc',
            contents: wranglerConfig + '\n',
          },
          {
            // Workers static assets read _headers from the asset directory;
            // hashed client bundles are immutable.
            kind: 'file',
            path: 'deploy/cloudflare/assets/_headers',
            contents: '/client/*\n  Cache-Control: public, max-age=31536000, immutable\n',
          },
          ...(options.projectConfig === false
            ? []
            : [
                {
                  kind: 'file',
                  path: 'wrangler.jsonc',
                  scope: 'project',
                  skipIfExists: true,
                  contents: projectWranglerConfig + '\n',
                } satisfies AdapterArtifact,
              ]),
        ],
      }
    },
  }
}

export default cloudflareAdapter
