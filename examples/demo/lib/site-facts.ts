/**
 * A module outside `app/`, reached through the `~/*` alias.
 *
 * It exists to be imported, and the import is the point. `examples/demo`
 * declared `"~/*": ["./*"]` in its tsconfig and then never used it, so the
 * alias-resolution rule both module graphs implement was covered by unit tests
 * and by nothing that runs end to end. This file makes `ruvyxa build`,
 * `ruvyxa dev` and `ruvyxa test:parity` exercise it on every run.
 *
 * It sits outside `app/` deliberately. That is the second rule it covers: a
 * build stages the application into `<out>/server/`, and a module the staging
 * copy does not contain cannot be resolved at request time — the failure that
 * used to answer a page with `RUV1801 cannot resolve '../../lib/x'`, naming a
 * path under `.ruvyxa` the author never wrote.
 */
export const SITE_FACTS = {
  routingModel: 'file-system',
  aliasPrefix: '~/',
} as const
