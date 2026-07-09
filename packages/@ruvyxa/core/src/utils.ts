import type { BuildContext } from './types.js'

export function validateBuildContext(
  ctx: BuildContext,
  adapterName: string,
): asserts ctx is BuildContext {
  if (!ctx.root || typeof ctx.root !== 'string') {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.root is required and must be a non-empty string`,
    )
  }
  if (!ctx.outDir || typeof ctx.outDir !== 'string') {
    throw new Error(
      `[RUV2000] ${adapterName}: BuildContext.outDir is required and must be a non-empty string`,
    )
  }
}
