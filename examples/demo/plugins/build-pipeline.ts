import { definePlugin } from 'ruvyxa/config'

/** Demonstrates module resolution, source transforms, and build lifecycle hooks. */
export default definePlugin({
  name: 'demo-build-pipeline',

  setup({ resolveId, transform, onBuildComplete }) {
    resolveId((id, _importer, context) => {
      if (id !== '~demo-plugin') return undefined
      const root = context.root.replaceAll('/', '\\')
      return `${root}\\plugins\\virtual-message.ts`
    })

    transform((code, id, context) => {
      const normalizedId = id.replaceAll('\\', '/')
      if (
        context.environment !== 'client' ||
        !normalizedId.endsWith('/plugin-lab/plugin-marker.ts')
      ) {
        return undefined
      }

      return code.replace("'original'", "'transformed-by-plugin'")
    })

    onBuildComplete(({ manifest }) => {
      console.info(
        `[demo-build-pipeline] completed a build with resolveId, transform, and lifecycle hooks (${JSON.stringify(manifest).length} manifest characters)`,
      )
    })
  },
})
