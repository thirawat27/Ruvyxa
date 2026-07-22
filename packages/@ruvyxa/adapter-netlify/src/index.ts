import type { Adapter, AdapterArtifact, AdapterOutput, BuildContext } from '@ruvyxa/core'
import { clientBuildOutput, validateBuildContext } from '@ruvyxa/core'

/**
 * Options for Netlify deployment.
 */
export interface NetlifyAdapterOptions {
  /** Custom Netlify functions directory. Defaults to `${outDir}/netlify/functions`. */
  functionsDir?: string
  /**
   * Also emit a `netlify.toml` at the project root pointing Netlify at the
   * generated publish directory and functions, so a fresh site deploys
   * without dashboard configuration. An existing project `netlify.toml` is
   * never overwritten.
   * @default true
   */
  projectConfig?: boolean
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
import { readFileSync, writeFileSync, mkdirSync, statSync } from 'node:fs';
import path from 'node:path';

const manifestPath = path.join(import.meta.dirname, 'manifest.json');
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
const prerenderDir = path.join(import.meta.dirname, 'prerender');

const handler = createHandler({
  routes: manifest.routes,
  importPage: loadRouteModule,
  importApi: loadRouteModule,
  readPrerendered: (pathname, revalidate = 60) => {
    // prerenderRelativePath rejects any request path that cannot be mapped to a
    // location inside prerenderDir, so the cache read can never escape it.
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return null;
    try {
      const htmlPath = path.join(prerenderDir, relative);
      const html = readFileSync(htmlPath, 'utf8');
      const stale = Date.now() - statSync(htmlPath).mtimeMs >= revalidate * 1000;
      return { html, stale };
    } catch {
      return null;
    }
  },
  writePrerendered: (pathname, html, revalidate) => {
    const relative = prerenderRelativePath(pathname);
    if (relative === null) return;
    const htmlPath = path.join(prerenderDir, relative);
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

      // Netlify.toml for the deploy directory
      const netlifyToml =
        '[build]\n  publish = "publish"\n  functions = "functions"\n\n' +
        '[[headers]]\n  for = "/client/*"\n  [headers.values]\n' +
        '    Cache-Control = "public, max-age=31536000, immutable"\n'

      // Project-root netlify.toml pointing at the build output
      const projectNetlifyToml =
        '[build]\n' +
        '  command = "npx --no-install ruvyxa build"\n' +
        `  publish = "${ctx.outDir}/deploy/netlify/publish"\n` +
        `  functions = "${ctx.outDir}/deploy/netlify/functions"\n\n` +
        '[[headers]]\n  for = "/client/*"\n  [headers.values]\n' +
        '    Cache-Control = "public, max-age=31536000, immutable"\n'

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
          // Static assets (pre-rendered pages + client bundles)
          { kind: 'static-site', path: 'deploy/netlify/publish' },
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
          ...(options.projectConfig === false
            ? []
            : [
                {
                  kind: 'file',
                  path: 'netlify.toml',
                  scope: 'project',
                  skipIfExists: true,
                  contents: projectNetlifyToml,
                } satisfies AdapterArtifact,
              ]),
        ],
      }
    },
  }
}

export default netlifyAdapter
