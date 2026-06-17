import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

export interface BunAdapterOptions {
  entry?: string
}

export function bunAdapter(options: BunAdapterOptions = {}): Adapter {
  return {
    name: "bun",
    target: "node",
    build(ctx: BuildContext): AdapterOutput {
      return {
        name: "bun",
        target: "node",
        platform: "bun",
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
      }
    },
  }
}

export default bunAdapter
