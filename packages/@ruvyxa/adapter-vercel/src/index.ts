import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

export interface VercelAdapterOptions {
  functionsDir?: string
}

export function vercelAdapter(options: VercelAdapterOptions = {}): Adapter {
  return {
    name: "vercel",
    target: "serverless",
    build(ctx: BuildContext): AdapterOutput {
      const functionsDir = options.functionsDir ?? `${ctx.outDir}/functions`
      return {
        name: "vercel",
        target: "serverless",
        platform: "vercel",
        entry: `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
        functionsDir,
        configFiles: ["vercel.json"],
      }
    },
  }
}

export default vercelAdapter
