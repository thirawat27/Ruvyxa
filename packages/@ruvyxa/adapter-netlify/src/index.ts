import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import {
  CLIENT_BUNDLE_PREFIX,
  clientBuildOutput,
  IMMUTABLE_CACHE_CONTROL,
  projectRelativeOutDir,
  PUBLIC_ASSET_CACHE_CONTROL,
  publicAssetGlobs,
  validateBuildContext,
} from '@ruvyxa/core'

/**
 * Options for Netlify deployment.
 */
export interface NetlifyAdapterOptions {
  /** Custom Netlify functions directory. Defaults to `${outDir}/netlify/functions`. */
  functionsDir?: string
  /**
   * Also emit a `netlify.toml` at the project root pointing Netlify at the
   * generated publish directory and functions. An existing project
   * `netlify.toml` is never overwritten.
   *
   * Off by default: the adapter already emits the serverless function and
   * cache headers through Netlify's Frameworks API (`.netlify/v1/`, a
   * gitignored build artifact), so the only remaining setup is the publish
   * directory — set once in the Netlify dashboard, or opt into this file.
   * @default false
   */
  projectConfig?: boolean
  /**
   * Emit the Netlify Frameworks API directory (`.netlify/v1/`) at the project
   * root during `ruvyxa build`: the SSR/API function and the immutable cache
   * headers for hashed client bundles. Netlify picks the directory up
   * automatically on the next deploy — no config file at the project root.
   * @default true
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
function netlifyHandlerSource(): string {
  return `import { createHandler, prerenderRelativePath } from './serverless-handler.mjs';
import { loadRouteModule } from './route-modules.mjs';
// Netlify bundles the function with esbuild, so anything the deployed code
// needs must be reachable through the module graph. A sibling manifest.json
// read from import.meta.dirname is not, and never reaches /var/task.
import manifest from './manifest.mjs';
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import os from 'node:os';
import path from 'node:path';

// Deploy-time prerender output is likewise dropped by the bundler unless the
// site declares included_files. Reads are best-effort: a miss falls through to
// an on-demand render, and Netlify serves SSG pages from the publish directory
// before the function is ever invoked (config.preferStatic).
const prerenderDir = path.join(import.meta.dirname, 'prerender');
// The function bundle directory is read-only at runtime; only the platform
// tmp directory accepts writes. ISR revalidations land there and are read
// back before the bundled deploy-time prerender output.
const isrCacheDir = path.join(os.tmpdir(), 'ruvyxa-isr-cache');

const readEntry = (htmlPath, revalidate) => {
  const html = readFileSync(htmlPath, 'utf8');
  const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
  return { html, stale };
};

const handler = createHandler({
  routes: manifest.routes,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
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
  writePrerendered: (pathname, html, revalidate) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(isrCacheDir, relative);
    try {
      mkdirSync(path.dirname(htmlPath), { recursive: true });
      writeFileSync(htmlPath, html, 'utf8');
    } catch {
      // ISR cache write failures are non-fatal
    }
  },
  supportedStrategies: ['ssr', 'ssg', 'csr', 'isr', 'ppr', 'api'],
});

// Netlify Functions v2 — Web-standard Request/Response
export default async function(request, context) {
  return handler(request, context);
}

// Netlify Functions v2 config
export const config = {
  path: '/*',
  preferStatic: true,
};
`
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
 * import { netlifyAdapter } from "@ruvyxa/adapter-netlify"
 *
 * export default config({
 *   adapter: netlifyAdapter()
 * })
 * ```
 */
export function netlifyAdapter(options: NetlifyAdapterOptions = {}): Adapter {
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
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/netlify/functions`
      // Config files are committed or read on other machines; never embed the
      // absolute build-machine outDir in them.
      const relativeOutDir = projectRelativeOutDir(ctx)

      // Hashed client bundles are served under /__ruvyxa/client/ (see the
      // chunk manifest src fields); the immutable header must match that URL.
      // `public/` assets are not hashed and otherwise inherit Netlify's
      // revalidate-every-request default, so they get the same lifetime the
      // Rust server sends for the same files.
      const headerRules: Array<{ for: string; cacheControl: string }> = [
        { for: `${CLIENT_BUNDLE_PREFIX}*`, cacheControl: IMMUTABLE_CACHE_CONTROL },
        ...publicAssetGlobs().map((glob) => ({
          for: glob,
          cacheControl: PUBLIC_ASSET_CACHE_CONTROL,
        })),
      ]

      const immutableHeaderToml = headerRules
        .map(
          (rule) =>
            `[[headers]]\n  for = "${rule.for}"\n  [headers.values]\n` +
            `    Cache-Control = "${rule.cacheControl}"\n`,
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
          headers: headerRules.map((rule) => ({
            for: rule.for,
            values: { 'Cache-Control': rule.cacheControl },
          })),
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
            excludeStrategies: ['isr', 'ppr'],
          },
          // Serverless function bundle
          {
            kind: 'function',
            path: 'deploy/netlify/functions/ruvyxa-handler',
            handlerSource: netlifyHandlerSource(),
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
          ...(options.frameworksApi === false
            ? []
            : [
                {
                  kind: 'function',
                  path: '.netlify/v1/functions/ruvyxa-handler',
                  scope: 'project',
                  handlerSource: netlifyHandlerSource(),
                } satisfies AdapterArtifact,
                {
                  kind: 'file',
                  path: '.netlify/v1/config.json',
                  scope: 'project',
                  contents: frameworksApiConfig + '\n',
                } satisfies AdapterArtifact,
              ]),
          ...(options.projectConfig === true
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

export default netlifyAdapter
