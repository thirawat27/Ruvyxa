import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

export interface StaticAdapterOptions {
  outputDir?: string
}

export function staticAdapter(options: StaticAdapterOptions = {}): Adapter {
  return {
    name: "static",
    target: "static",
    build(ctx: BuildContext): AdapterOutput {
      return {
        name: "static",
        target: "static",
        platform: "static",
        entry: options.outputDir ?? `${ctx.outDir}/static`,
        assetsDir: `${ctx.outDir}/assets`,
      }
    },
  }
}

export default staticAdapter
