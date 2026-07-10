import {
  type User,
  createUser,
} from './user.js'
import './side-effect.js'

export {
  createHelper,
} from './helper.js'

export const loadLazy = () => import(
  './lazy.js'
)
export const loadData = () => require(
  './data.cjs'
)

const stringExample = "import('./string-only.js')"
const object = { require(value: string) { return value } }
object.require('./member-call.js')
// import('./line-comment.js')
/* require('./block-comment.cjs') */

export { stringExample, object, createUser }
