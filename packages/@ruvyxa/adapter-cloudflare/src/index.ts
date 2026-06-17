import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

export interface CloudflareAdapterOptions {
  workerEntry?: string
}

export function cloudflareAdapter(options: CloudflareAdapterOptions = {}): Adapter {
  return {
    name: "cloudflare",
    target: "edge",
    build(ctx: BuildContext): AdapterOutput {
      return {
        name: "cloudflare",
        target: "edge",
        platform: "cloudflare",
        entry: options.workerEntry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        configFiles: ["wrangler.toml"],
      }
    },
  }
}

export default cloudflareAdapter
