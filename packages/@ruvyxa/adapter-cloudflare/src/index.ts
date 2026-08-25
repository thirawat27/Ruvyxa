import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  clientBuildOutput,
  headersFileContents,
  projectRelativeOutDir,
  runtimeBuildPolicy,
  validateBuildContext,
} from '@ruvyxa/core'

/**
 * Options for the Cloudflare Workers deployment adapter.
 */
export interface CloudflareAdapterOptions {
  /** Custom worker entry point path. Defaults to `${outDir}/server/app`. */
  workerEntry?: string
  /**
   * Also emit a `wrangler.jsonc` at the project root pointing at the
   * generated Worker script and static assets. An existing project
   * `wrangler.jsonc` is never overwritten.
   *
   * Off by default: the deploy directory already contains a self-sufficient
   * config, so `wrangler deploy -c .ruvyxa/deploy/cloudflare/wrangler.jsonc`
   * works with no file at the project root.
   * @default false
   */
  projectConfig?: boolean
  /**
   * Cloudflare Workers compatibility date, which pins the runtime API
   * behavior the Worker was written against.
   *
   * Defaults to a fixed date this adapter is tested with, never the current
   * date: a build-time date makes two builds of the same commit differ, and a
   * date newer than the workerd version on the deploy machine is rejected by
   * `wrangler`. Raise it deliberately after checking Cloudflare's changelog.
   */
  compatibilityDate?: string
  /**
   * Workers KV namespace binding that stores revalidated documents, which is
   * what lets this adapter serve ISR and PPR.
   *
   * Off by default, and the capability follows it: with no binding the adapter
   * declares it does not support `isr` or `ppr`, so a project using either is
   * refused at build time with `RUV2202` rather than deployed to a Worker that
   * would re-render every request and call it a cache.
   *
   * Create the namespace once (`wrangler kv namespace create RUVYXA_ISR`), then
   * name the binding here. The generated `wrangler.jsonc` declares it, but the
   * namespace id is the project's to fill in — it is account-specific and must
   * not be baked into a generated file.
   *
   * ```ts
   * cloudflareAdapter({ isr: { kvBinding: 'RUVYXA_ISR' } })
   * ```
   */
  isr?: { kvBinding: string }
}

/**
 * Compatibility date this adapter's generated Worker is verified against.
 *
 * Bump only together with a check of the Workers runtime changes it opts into.
 */
const DEFAULT_COMPATIBILITY_DATE = '2025-09-01'

/**
 * Node.js compatibility, which this Worker genuinely depends on.
 *
 * `runtime/request-context.mjs` reaches for `node:async_hooks` and falls back
 * to a single-slot store when the runtime has none. That fallback refuses a
 * second concurrent render rather than risk serving one visitor's page to
 * another, so on Workers every app that calls `cookies()`, `headers()`, or
 * `draftMode()` while rendering failed as soon as two requests overlapped.
 *
 * Cloudflare enables the Node.js APIs without a flag only from compatibility
 * date `2026-08-04`; between `2024-09-23` and that date they are behind
 * `nodejs_compat`, which is the range `DEFAULT_COMPATIBILITY_DATE` sits in.
 * The flag stays harmless once the date is raised past the cutoff.
 */
const COMPATIBILITY_FLAGS = ['nodejs_compat']

/**
 * The `kv_namespaces` stanza a wrangler config carries when ISR is configured.
 *
 * `id` is left as a placeholder on purpose: a namespace id belongs to one
 * Cloudflare account, so baking a real one into a generated file would either
 * leak it or hand every reader a namespace that is not theirs. `wrangler kv
 * namespace create <binding>` prints the id to paste in.
 */
function kvNamespaces(kvBinding: string | null) {
  if (!kvBinding) return {}
  return {
    kv_namespaces: [
      { binding: kvBinding, id: '<run: wrangler kv namespace create ' + kvBinding + '>' },
    ],
  }
}

/**
 * The strategies a Worker can answer, which is decided by whether the project
 * gave this adapter somewhere to store a revalidated document.
 *
 * Declared rather than guessed. Claiming ISR with no store would deploy a
 * Worker that re-renders every request and reports `x-ruvyxa-isr: HIT`, which
 * is worse than refusing the build.
 */
function workerStrategies(kvBinding: string | null): Adapter['supports'] {
  const base: Adapter['supports'] = ['ssr', 'ssg', 'csr', 'api']
  return kvBinding ? [...base, 'isr', 'ppr'] : base
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
function workerHandlerSource(runtimePolicy: unknown, kvBinding: string | null): string {
  return `import { createHandler } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
// A JS module, not a JSON import: import attributes for JSON are not uniformly
// available across bundlers and Worker compatibility dates.
import manifest from './manifest.mjs';

const runtimePolicy = ${JSON.stringify(runtimePolicy ?? {})};

/**
 * The KV namespace revalidated documents are stored in, once a request has
 * handed the bindings over. Null when the project configured no namespace, in
 * which case the adapter also declared it does not support ISR or PPR, so
 * nothing reaches the reader.
 */
let isrStore = null;

/**
 * How much longer than its revalidation window a document is kept.
 *
 * ISR answers from a stale copy while it refreshes behind the response, so the
 * entry has to outlive the moment it goes stale — dropping it on the TTL would
 * make every refresh a blocking render.
 */
const STALE_RETENTION_FACTOR = 10;

/** A document's key. Prefixed so the namespace can hold other things safely. */
function isrKey(pathname) {
  return \`isr:\${pathname}\`;
}

async function optimizeImage(request, { src, width, quality }) {
  if (width > (runtimePolicy.image?.maxWidth ?? 3840)) {
    return new Response('Image width exceeds configured maximum', { status: 400 });
  }
  const source = new URL(src, request.url);
  const transformed = await fetch(source, {
    cf: { image: { width, quality, fit: 'scale-down', format: 'auto' } },
  });
  const headers = new Headers(transformed.headers);
  headers.set('cache-control', 'public, max-age=86400, stale-while-revalidate=604800');
  return new Response(transformed.body, {
    status: transformed.status,
    statusText: transformed.statusText,
    headers,
  });
}

const handler = createHandler({
  routes: manifest.routes,
  middleware: runtimePolicy.middleware,
  i18n: manifest.i18n,
  optimizeImage: runtimePolicy.image?.onDemand === true ? optimizeImage : undefined,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  importAction: loadActionModule,
  pluginHttp: applyPluginHttp,
  security: runtimePolicy.security,
  readPrerendered: async (pathname, revalidate = 60) => {
    // A Worker has no filesystem, so the store is KV and the read is async —
    // which is the whole reason this returned \`null\` before \`readPrerendered\`
    // was allowed to be asynchronous.
    if (!isrStore) return null;
    const entry = await isrStore.getWithMetadata(isrKey(pathname), { type: 'text' });
    if (entry == null || entry.value == null) return null;
    const storedAt = Number(entry.metadata && entry.metadata.storedAt);
    // An entry with no usable stamp is stale rather than fresh: serving it is
    // still correct, and it schedules the refresh that replaces it.
    const fresh = Number.isFinite(storedAt) && Date.now() - storedAt < revalidate * 1000;
    return { html: entry.value, stale: !fresh };
  },
  writePrerendered: async (pathname, html, revalidate = 60) => {
    if (!isrStore) return;
    await isrStore.put(isrKey(pathname), html, {
      metadata: { storedAt: Date.now() },
      // Kept well past the revalidation window on purpose. Serving a stale
      // document while refreshing behind it is what ISR *is*, so expiring the
      // entry the moment it goes stale would turn every refresh into a blocking
      // render — the opposite of the strategy. KV's own floor is 60 seconds.
      expirationTtl: Math.max(60, Math.round(revalidate * STALE_RETENTION_FACTOR)),
    });
  },
  // The project's own not-found page, pre-rendered by the build and carried
  // inline in the manifest: an unmatched URL is answered with the page the
  // application actually wrote, on every host.
  notFoundDocument: manifest.notFoundDocument,
  supportedStrategies: ${JSON.stringify(workerStrategies(kvBinding))},
});

export default {
  async fetch(request, env, ctx) {
    // Bindings arrive with the request while the handler is built once at
    // module scope, so the store is captured here and read by the closures
    // above. An isolate serves many requests; this assignment is idempotent.
    isrStore = ${kvBinding ? `env.${kvBinding} ?? null` : 'null'};
    // The runtime context carries waitUntil, which the shared handler uses to
    // finish background work after the response is returned.
    return handler(request, ctx);
  },
};
`
}

/**
 * Create a Cloudflare Workers deployment adapter for Ruvyxa.
 *
 * Produces a Worker fetch handler and static assets for deployment via
 * `wrangler`. SSR, API routes, SSG, and CSR always work.
 *
 * ISR and PPR need somewhere to keep a revalidated document, and a Worker has
 * no filesystem — so they are available exactly when the project names a
 * Workers KV binding through `isr.kvBinding`, and refused with `RUV2202` when
 * it does not. The capability is declared from the option rather than assumed,
 * because a Worker that re-renders every request while reporting a cache hit is
 * worse than a build that stops.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { cloudflare } from "@ruvyxa/adapter-cloudflare"
 *
 * export default config({
 *   adapter: cloudflare()
 * })
 * ```
 */
export function cloudflare(options: CloudflareAdapterOptions = {}): Adapter {
  if (options.workerEntry !== undefined && typeof options.workerEntry !== 'string') {
    throw new Error(
      `[RUV2001] cloudflareAdapter: "workerEntry" must be a string, got ${typeof options.workerEntry}`,
    )
  }

  if (options.workerEntry !== undefined && options.workerEntry.trim() === '') {
    throw new Error(`[RUV2001] cloudflareAdapter: "workerEntry" must not be an empty string`)
  }

  const kvBinding = options.isr?.kvBinding ?? null
  if (options.isr !== undefined && (typeof kvBinding !== 'string' || kvBinding.trim() === '')) {
    throw new Error(
      `[RUV2001] cloudflareAdapter: "isr.kvBinding" must be a non-empty string naming a Workers KV binding`,
    )
  }
  // A binding is a JavaScript identifier on `env`, so a name that is not one
  // would emit a Worker that does not parse — caught here rather than by
  // `wrangler` after the build has already claimed success.
  if (kvBinding !== null && !/^[A-Za-z_$][\w$]*$/.test(kvBinding)) {
    throw new Error(
      `[RUV2001] cloudflareAdapter: "isr.kvBinding" must be a valid identifier, got ${JSON.stringify(kvBinding)}`,
    )
  }

  return {
    name: 'cloudflare',
    target: 'edge',
    supports: workerStrategies(kvBinding),
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'cloudflareAdapter')

      const compatDate = options.compatibilityDate ?? DEFAULT_COMPATIBILITY_DATE
      // Config files are committed or read on other machines; never embed the
      // absolute build-machine outDir in them.
      const relativeOutDir = projectRelativeOutDir(ctx)
      const runtimePolicy = runtimeBuildPolicy(ctx)

      const wranglerConfig = JSON.stringify(
        {
          name: 'ruvyxa-app',
          main: './worker/index.mjs',
          compatibility_date: compatDate,
          compatibility_flags: COMPATIBILITY_FLAGS,
          assets: { directory: './assets' },
          ...kvNamespaces(kvBinding),
        },
        null,
        2,
      )

      const projectWranglerConfig = JSON.stringify(
        {
          name: 'ruvyxa-app',
          main: `${relativeOutDir}/deploy/cloudflare/worker/index.mjs`,
          compatibility_date: compatDate,
          compatibility_flags: COMPATIBILITY_FLAGS,
          assets: { directory: `${relativeOutDir}/deploy/cloudflare/assets` },
          ...kvNamespaces(kvBinding),
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
          // Static assets served by Cloudflare's asset binding. `optional`:
          // an API-only or all-SSR app has no prerendered pages; the Worker
          // still serves every route, so the missing prerender directory must
          // not fail the build.
          { kind: 'static-site', path: 'deploy/cloudflare/assets', optional: true },
          // Worker function bundle (SSR/API handler)
          {
            kind: 'function',
            path: 'deploy/cloudflare/worker',
            handlerSource: workerHandlerSource(runtimePolicy, kvBinding),
          },
          // Wrangler config pointing at the Worker + assets
          {
            kind: 'file',
            path: 'deploy/cloudflare/wrangler.jsonc',
            contents: wranglerConfig + '\n',
          },
          {
            // Workers static assets read _headers from the asset directory.
            // Hashed client bundles are immutable; `public/` assets otherwise
            // inherit Cloudflare's `max-age=0, must-revalidate` default, which
            // re-fetches every image and font on each navigation.
            kind: 'file',
            path: 'deploy/cloudflare/assets/_headers',
            contents: headersFileContents(),
          },
          ...(options.projectConfig === true
            ? [
                {
                  kind: 'file',
                  path: 'wrangler.jsonc',
                  scope: 'project',
                  skipIfExists: true,
                  contents: projectWranglerConfig + '\n',
                } satisfies AdapterArtifact,
              ]
            : []),
        ],
      }
    },
  }
}

export default cloudflare
