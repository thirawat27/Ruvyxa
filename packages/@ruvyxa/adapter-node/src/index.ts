import type { Adapter, AdapterOutput, BuildContext } from "@ruvyxa/core"

export interface NodeAdapterOptions {
  entry?: string
}

export function nodeAdapter(options: NodeAdapterOptions = {}): Adapter {
  return {
    name: "node",
    target: "node",
    build(ctx: BuildContext): AdapterOutput {
      return {
        name: "node",
        target: "node",
        entry: options.entry ?? `${ctx.outDir}/server/app`,
        assetsDir: `${ctx.outDir}/assets`,
      }
    },
  }
}

export default nodeAdapter
