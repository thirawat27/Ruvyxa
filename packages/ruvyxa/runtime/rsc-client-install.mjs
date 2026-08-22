/**
 * Install the client-reference globals, as a side effect of being imported.
 *
 * This file exists because of one fact about
 * `react-server-dom-webpack/client.browser`: it reads `__webpack_require__.u`
 * while its own module body runs, so the globals have to be in place before it
 * is evaluated — not before it is *called*. A browser entry therefore imports
 * this module first, and the linker, which evaluates a module's dependencies in
 * the order they are imported, does the rest.
 *
 * It is a separate file rather than a line at the bottom of
 * `rsc-client-runtime.mjs` because defining `__webpack_require__` is a claim
 * about the realm that other libraries read: `sass` tests
 * `typeof __webpack_require__` and then reaches for `__non_webpack_require__`.
 * Making that claim a side effect of importing the module that owns Ruvyxa's
 * reference registry broke every SCSS build in the same process, because
 * `compiler.mjs` reaches that module to compute reference ids and never intends
 * to be inside webpack. Splitting the side effect out is what lets the server
 * import the implementation without making the claim.
 */

import { installClientReferenceRuntime } from './rsc-client-runtime.mjs'

installClientReferenceRuntime()

export { installClientReferenceRuntime }
