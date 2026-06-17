import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

export interface NetlifyAdapterOptions {
  functionsDir?: string
}

export function netlifyAdapter(options: NetlifyAdapterOptions = {}): Adapter {
  return {
    name: "netlify",
    target: "serverless",
    build(ctx: BuildContext): AdapterOutput {
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/netlify/functions`
      return {
        name: "netlify",
        target: "serverless",
        platform: "netlify",
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        functionsDir,
        configFiles: ["netlify.toml"],
      }
    },
  }
}

export default netlifyAdapter
