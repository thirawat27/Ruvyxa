import { loader, cache } from 'ruvyxa/server'

/**
 * Task data model.
 * In a real application, this would come from a database (e.g. Postgres, SQLite).
 * The in-memory store here is for demonstration purposes only.
 */
export interface Task {
  id: string
  title: string
  done: boolean
  createdAt: number
}

// In-memory data store — resets on server restart.
// Replace with your preferred database in production.
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
 * Get all tasks, sorted by creation date (newest first).
 * Uses `cache()` so repeated reads within a request are deduplicated.
 */
export const getTasks = loader(() =>
  cache('tasks')
    .ttl('5m')
    .get(() => [...store.tasks].sort((a, b) => b.createdAt - a.createdAt)),
)

/**
 * Internal helpers used by server actions.
 */
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
  const idx = store.tasks.findIndex((task) => task.id === id)
  if (idx === -1) return false
  store.tasks.splice(idx, 1)
  return true
}
