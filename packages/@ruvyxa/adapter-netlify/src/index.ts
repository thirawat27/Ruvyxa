import { readFileSync } from 'node:fs'

import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  CLIENT_BUNDLE_PREFIX,
  clientBuildOutput,
  DEFAULT_SECURITY_HEADERS,
  IMMUTABLE_CACHE_CONTROL,
  nonPublishableStrategies,
  projectRelativeOutDir,
  PUBLIC_ASSET_CACHE_CONTROL,
  publicAssetGlobs,
  runtimeBuildPolicy,
  validateBuildContext,
} from '@ruvyxa/core'

/**
 * Options for Netlify deployment.
 */
export interface NetlifyAdapterOptions {
  /** Custom Netlify functions directory. Defaults to `${outDir}/netlify/functions`. */
  functionsDir?: string
  /**
   * Emit a `netlify.toml` at the project root naming the build command, the
   * generated publish directory, the functions directory, and the headers. An
   * existing project `netlify.toml` is never overwritten.
   *
   * On by default, because it is the only mechanism that can answer the one
   * question Netlify cannot learn from the build: **the publish directory**.
   * The Frameworks API has no key for it, and a plugin cannot supply it without
   * a `netlify.toml` of its own — Netlify installs a plugin from its UI only
   * when the plugin is listed in Netlify's own directory, and a community
   * package has to be declared in `[[plugins]]` and installed as a
   * devDependency. So the choice is not "config file or no config file"; it is
   * "one generated file, or the same file written by hand".
   *
   * Netlify reads `netlify.toml` *before* running the build command, so this
   * file has to be committed to take effect — the build writes it once and
   * never again.
   * @default true
   */
  projectConfig?: boolean
  /**
   * Emit the Netlify Frameworks API directory (`.netlify/v1/`) at the project
   * root during `ruvyxa build`: the SSR/API function and the cache headers.
   *
   * Defaults to the opposite of {@link projectConfig}, because the two are
   * alternatives rather than layers. Shipping both puts two functions on the
   * site — the one under `.netlify/v1/functions` and the one the `netlify.toml`
   * `functions` directory declares — and both claim `path: '/*'`, so which of
   * them answers a request is not a question with an answer.
   * @default `!projectConfig`
   */
  frameworksApi?: boolean
}

/**
 * Netlify Functions v2 handler source code.
 *
 * Wraps the generic Ruvyxa serverless handler into a Netlify Functions v2
 * handler using the Web-standard Request/Response API. Reads the route
 * manifest and handles SSR/API/ISR/PPR requests.
 */
function netlifyHandlerSource(
  runtimePolicy: unknown,
  functionConfig: NetlifyFunctionConfig,
): string {
  return `import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { applyPluginHttp, loadActionModule, loadRouteModule } from './route-modules.mjs';
// Netlify bundles the function with esbuild, so anything the deployed code
// needs must be reachable through the module graph. A sibling manifest.json
// read from import.meta.dirname is not, and never reaches /var/task.
import manifest from './manifest.mjs';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const runtimePolicy = ${JSON.stringify(runtimePolicy ?? {})};

// Deploy-time prerender output is not part of the module graph, so it reaches
// the bundle through the includedFiles glob in the config below. Reads stay
// best-effort anyway: a miss falls through to an on-demand render, and Netlify
// serves SSG pages from the publish directory before the function is ever
// invoked (config.preferStatic).
const prerenderDir = path.join(import.meta.dirname, 'prerender');
// The function bundle directory is read-only at runtime; only the platform
// tmp directory accepts writes. ISR revalidations land there and are read
// back before the bundled deploy-time prerender output.
const isrCacheDir = path.join(os.tmpdir(), 'ruvyxa-isr-cache');

/**
 * The cache tag a document is stored under, so one path can be dropped from
 * Netlify's cache without dropping the site's.
 *
 * A tag list is comma-separated, so the path is folded to one token.
 */
function cacheTag(pathname) {
  return 'ruvyxa' + pathname.replace(/[^A-Za-z0-9]+/g, '-').replace(/-+$/, '');
}

/**
 * Drop this path from Netlify's edge, which the durable cache in front of this
 * function would otherwise keep serving until its window expired.
 *
 * \`purgeCache\` infers the site and its credentials from the function runtime,
 * so it needs no configuration — but it lives in \`@netlify/functions\`, which a
 * project may not have installed. Imported at the moment it is needed and
 * behind a catch, because a missing optional dependency must not take the whole
 * function down at module load: every page would 500 to make one revalidation
 * work.
 */
async function purgeDurableCache(pathname) {
  try {
    const { purgeCache } = await import('@netlify/functions');
    await purgeCache({ tags: [cacheTag(pathname)] });
  } catch (error) {
    console.error('[ruvyxa] could not purge ' + pathname + ' from the Netlify cache:', error);
  }
}

const readEntry = (htmlPath, revalidate) => {
  const html = readFileSync(htmlPath, 'utf8');
  const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
  return { html, stale };
};

const handler = createHandler({
  routes: manifest.routes,
  middleware: runtimePolicy.middleware,
  i18n: manifest.i18n,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  importAction: loadActionModule,
  pluginHttp: applyPluginHttp,
  security: runtimePolicy.security,
  readPrerendered: (pathname, revalidate = 60) => {
    // prerenderRelativePath rejects any request path that cannot be mapped to a
    // location inside the cache directories, so reads can never escape them.
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return null;
    try {
      return readEntry(path.join(isrCacheDir, relative), revalidate);
    } catch {
      // fall through to the bundled prerender output
    }
    try {
      return readEntry(path.join(prerenderDir, relative), revalidate);
    } catch {
      return null;
    }
  },
  writePrerendered: (pathname, html, revalidate, forced) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(isrCacheDir, relative);
    mkdirSync(path.dirname(htmlPath), { recursive: true });
    writeFileSync(htmlPath, html, 'utf8');
    if (forced === true) return purgeDurableCache(pathname);
  },
  // The project's own not-found page, pre-rendered by the build and carried
  // inline in the manifest: an unmatched URL is answered with the page the
  // application actually wrote, on every host.
  notFoundDocument: manifest.notFoundDocument,
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

// Netlify Functions v2 — Web-standard Request/Response
export default async function(request, context) {
  const response = await handler(request, context);
  return withDurableCache(request, response);
}

/**
 * Re-state a cacheable answer for Netlify's own cache, durably.
 *
 * \`Cache-Control\` reaches the browser and every CDN; \`Netlify-CDN-Cache-Control\`
 * is read by Netlify alone and is the only place the \`durable\` directive means
 * anything. Durable storage is what makes one render serve every edge node
 * instead of one — and it is what \`purgeCache\` above can invalidate, so the two
 * halves have to be added together or \`revalidatePath()\` purges nothing.
 *
 * Only a shared-cacheable response is re-stated: \`s-maxage\` is the marker the
 * handler already puts on an ISR document, and \`no-store\` on everything a
 * request produced for one visitor.
 */
function withDurableCache(request, response) {
  const cacheControl = response.headers.get('cache-control');
  if (!cacheControl || !cacheControl.includes('s-maxage')) return response;
  const headers = new Headers(response.headers);
  headers.set('netlify-cdn-cache-control', 'public, durable, ' + cacheControl);
  headers.set('cache-tag', cacheTag(new URL(request.url).pathname));
  return new Response(response.body, {
    status: response.status,
    statusText: response.statusText,
    headers,
  });
}

// Netlify Functions v2 config
export const config = ${JSON.stringify(functionConfig, null, 2)};
`
}

/**
 * The Functions v2 config this adapter declares.
 *
 * `name` and `generator` are the Frameworks API's attribution fields: Netlify
 * shows the name in the site UI and reads the generator to tell a
 * framework-emitted function from a hand-written one.
 *
 * `includedFiles` is what carries the deploy-time prerender output into the
 * bundle. Netlify bundles the function with esbuild, which copies only what the
 * module graph reaches, so the HTML `readPrerendered` falls back to was being
 * dropped. The globs resolve against the site's base directory, which is why
 * each artifact declares its own path instead of sharing one.
 */
interface NetlifyFunctionConfig {
  path: string
  preferStatic: true
  name: string
  generator: string
  includedFiles: string[]
}

/** Read the adapter package version so `generator` carries real semver. */
function packageVersion(): string {
  const metadata = JSON.parse(
    readFileSync(new URL('../package.json', import.meta.url), 'utf8'),
  ) as {
    version?: unknown
  }
  if (typeof metadata.version !== 'string' || !/^\d+\.\d+\.\d+/.test(metadata.version)) {
    throw new Error('[RUV2001] netlifyAdapter: package version must be valid semver metadata')
  }
  return metadata.version
}

function functionConfigFor(functionRoot: string): NetlifyFunctionConfig {
  return {
    path: '/*',
    preferStatic: true,
    name: 'Ruvyxa SSR',
    generator: `ruvyxa/${packageVersion()}`,
    includedFiles: [`${functionRoot}/prerender/**`],
  }
}

/**
 * Create a Netlify deployment adapter for Ruvyxa.
 *
 * Produces a Functions v2 handler (Web-standard Request/Response) and static
 * assets for deployment via Netlify CLI. Supports SSR, API routes, ISR, PPR,
 * SSG, and CSR.
 *
 * @example
 * ```ts
 * import { config } from "ruvyxa/config"
 * import { netlify } from "@ruvyxa/adapter-netlify"
 *
 * export default config({
 *   adapter: netlify()
 * })
 * ```
 */
export function netlify(options: NetlifyAdapterOptions = {}): Adapter {
  if (options.functionsDir !== undefined && typeof options.functionsDir !== 'string') {
    throw new Error(
      `[RUV2001] netlifyAdapter: "functionsDir" must be a string, got ${typeof options.functionsDir}`,
    )
  }

  if (options.functionsDir !== undefined && options.functionsDir.trim() === '') {
    throw new Error(`[RUV2001] netlifyAdapter: "functionsDir" must not be an empty string`)
  }

  return {
    name: 'netlify',
    target: 'serverless',
    supports: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
    build(ctx: BuildContext): AdapterOutput {
      validateBuildContext(ctx, 'netlifyAdapter')
      // Exactly one of the two ships: see `frameworksApi` above for why both
      // at once is a site with two functions claiming the same path.
      const projectConfig = options.projectConfig !== false
      const frameworksApi = options.frameworksApi ?? !projectConfig
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/netlify/functions`
      // Config files are committed or read on other machines; never embed the
      // absolute build-machine outDir in them.
      const relativeOutDir = projectRelativeOutDir(ctx)
      const runtimePolicy = runtimeBuildPolicy(ctx)

      // Hashed client bundles are served under /__ruvyxa/client/ (see the
      // chunk manifest src fields); the immutable header must match that URL.
      // `public/` assets are not hashed and otherwise inherit Netlify's
      // revalidate-every-request default, so they get the same lifetime the
      // Rust server sends for the same files.
      //
      // The `/*` rule carries the security defaults, and it is first because
      // Netlify publishes pre-rendered documents and public files itself: the
      // function — where `createHandler` sets these headers — is never invoked
      // for them. Without it a deployed SSG page lost every header the same
      // page carries under `ruvyxa start`, including `X-Frame-Options`.
      const headerRules: Array<{ for: string; values: Record<string, string> }> = [
        { for: '/*', values: { ...DEFAULT_SECURITY_HEADERS } },
        {
          for: `${CLIENT_BUNDLE_PREFIX}*`,
          values: { 'Cache-Control': IMMUTABLE_CACHE_CONTROL },
        },
        ...publicAssetGlobs().map((glob) => ({
          for: glob,
          values: { 'Cache-Control': PUBLIC_ASSET_CACHE_CONTROL },
        })),
      ]

      const immutableHeaderToml = headerRules
        .map(
          (rule) =>
            `[[headers]]\n  for = "${rule.for}"\n  [headers.values]\n` +
            Object.entries(rule.values)
              .map(([name, value]) => `    ${name} = "${value}"\n`)
              .join(''),
        )
        .join('')

      // Netlify.toml for the deploy directory
      const netlifyToml =
        '[build]\n  publish = "publish"\n  functions = "functions"\n\n' + immutableHeaderToml

      // Project-root netlify.toml pointing at the build output (opt-in)
      const projectNetlifyToml =
        '[build]\n' +
        '  command = "npx --no-install ruvyxa build"\n' +
        `  publish = "${relativeOutDir}/deploy/netlify/publish"\n` +
        `  functions = "${relativeOutDir}/deploy/netlify/functions"\n\n` +
        immutableHeaderToml

      // Frameworks API deploy configuration (.netlify/v1/config.json)
      const frameworksApiConfig = JSON.stringify(
        {
          headers: headerRules.map((rule) => ({ for: rule.for, values: rule.values })),
        },
        null,
        2,
      )

      return {
        name: 'netlify',
        target: 'serverless',
        platform: 'netlify',
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        ...clientBuildOutput(ctx),
        functionsDir,
        configFiles: ['netlify.toml'],
        artifacts: [
          // Static assets (pre-rendered pages + client bundles). `optional`:
          // an API-only or all-SSR app has no prerendered pages; the function
          // still serves every route, so the missing prerender directory must
          // not fail the build.
          // `preferStatic: true` means a published page is served without ever
          // reaching the function, so ISR/PPR pages must stay unpublished for
          // revalidation to happen at all.
          {
            kind: 'static-site',
            path: 'deploy/netlify/publish',
            optional: true,
            excludeStrategies: nonPublishableStrategies(),
          },
          // Serverless function bundle
          {
            kind: 'function',
            path: 'deploy/netlify/functions/ruvyxa-handler',
            handlerSource: netlifyHandlerSource(
              runtimePolicy,
              functionConfigFor('functions/ruvyxa-handler'),
            ),
          },
          // Netlify config for the deploy directory
          {
            kind: 'file',
            path: 'deploy/netlify/netlify.toml',
            contents: netlifyToml,
          },
          // Frameworks API artifacts: Netlify discovers .netlify/v1/ at the
          // project root automatically, so a fresh site needs only the publish
          // directory set once in the dashboard — no committed config file.
          ...(frameworksApi
            ? [
                {
                  kind: 'function',
                  path: '.netlify/v1/functions/ruvyxa-handler',
                  scope: 'project',
                  handlerSource: netlifyHandlerSource(
                    runtimePolicy,
                    functionConfigFor('.netlify/v1/functions/ruvyxa-handler'),
                  ),
                } satisfies AdapterArtifact,
                {
                  kind: 'file',
                  path: '.netlify/v1/config.json',
                  scope: 'project',
                  contents: frameworksApiConfig + '\n',
                } satisfies AdapterArtifact,
              ]
            : []),
          ...(projectConfig
            ? [
                {
                  kind: 'file',
                  path: 'netlify.toml',
                  scope: 'project',
                  skipIfExists: true,
                  contents: projectNetlifyToml,
                } satisfies AdapterArtifact,
              ]
            : []),
        ],
      }
    },
  }
}

export default netlify
