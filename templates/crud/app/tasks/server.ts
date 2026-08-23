import { cache, loader } from 'ruvyxa/server'

/**
 * The task list's data access.
 *
 * ## Replace the store before this is a real application
 *
 * The `Map`-free array below lives in the server process, which means it resets
 * on restart and each worker or instance holds its own copy — `ruvyxa dev` runs
 * several. It is here so the starter has something to read and write without a
 * database; it is not a design to copy.
 *
 * ## Why this module is not marked `server-only`
 *
 * `page.tsx` imports `getTasks` from here, and a page is rendered in the browser
 * as well as on the server, so this module — the seed rows included — is part of
 * the client graph and ships in the route bundle. Adding `import 'server-only'`
 * makes the build fail with `RUV1007` rather than making the problem go away.
 *
 * The clean fix is `export const serverComponents = true` on `/tasks`, which
 * keeps the server half out of the browser entirely. It is deliberately not done
 * here: every deploy adapter refuses a *dynamic* server-components route with
 * `RUV2213`, so a starter that used it would scaffold applications that cannot
 * be deployed. Real data behind a database client — which does not resolve in a
 * browser at all — does not have this shape. Keep secrets out of this module,
 * and put anything that must never reach a browser in an API route or an action.
 */
export interface Task {
  id: string
  title: string
  done: boolean
  createdAt: number
}

interface TaskStore {
  tasks: Task[]
  nextId: number
}

const runtime = globalThis as typeof globalThis & { __RUVYXA_CRUD_TASKS__?: TaskStore }
const store = (runtime.__RUVYXA_CRUD_TASKS__ ??= {
  tasks: [
    { id: '1', title: 'Set up database connection', done: false, createdAt: Date.now() - 3600_000 },
    { id: '2', title: 'Add authentication', done: false, createdAt: Date.now() - 1800_000 },
    { id: '3', title: 'Deploy to production', done: true, createdAt: Date.now() - 900_000 },
  ],
  nextId: 4,
})

/**
 * Every task, newest first.
 *
 * `cache('tasks')` deduplicates reads inside a request and holds the answer for
 * five minutes across requests. The actions in `action.ts` call
 * `invalidate('tasks')` after every write, which is what keeps a cached list
 * from outliving the change that made it wrong.
 */
export const getTasks = loader(() =>
  cache('tasks')
    .ttl('5m')
    .get(() => [...store.tasks].sort((left, right) => right.createdAt - left.createdAt)),
)

/** Internal helpers, called only by the server actions in `action.ts`. */
export function addTask(title: string): Task {
  const task: Task = { id: String(store.nextId++), title, done: false, createdAt: Date.now() }
  store.tasks.push(task)
  return task
}

export function toggleTaskById(id: string): boolean {
  const task = store.tasks.find((candidate) => candidate.id === id)
  if (!task) return false
  task.done = !task.done
  return true
}

export function deleteTaskById(id: string): boolean {
  const index = store.tasks.findIndex((task) => task.id === id)
  if (index === -1) return false
  store.tasks.splice(index, 1)
  return true
}
