import type { Meta } from '@ruvyxa/react'

import { getTasks } from './server'

export const meta: Meta = {
  title: 'Tasks',
  description: 'A task list backed by server actions.',
}

/**
 * The endpoint a `<form>` posts a server action to.
 *
 * `path` is the route the action module belongs to and `name` is the export.
 * Because this is an ordinary form post rather than a `fetch`, every control on
 * this page keeps working with JavaScript turned off: the server runs the
 * action and answers with a new document.
 */
function actionUrl(name: string): string {
  return `/__ruvyxa/action?path=/tasks&name=${name}`
}

/**
 * Tasks — rendered on the server from the data loader, mutated by server
 * actions. Each action invalidates the `tasks` cache key, so the document the
 * browser gets back already reflects the write.
 */
export default async function TasksPage() {
  const tasks = await getTasks()

  return (
    <main>
      <h1>Tasks</h1>
      <p>Manage your task list. Every change is handled by a server action.</p>

      <form method="post" action={actionUrl('createTask')} aria-label="Add a new task">
        <input
          type="text"
          name="title"
          placeholder="What needs to be done?"
          required
          maxLength={200}
          aria-label="Task title"
          autoComplete="off"
        />
        <button type="submit">Add</button>
      </form>

      {tasks.length === 0 ? (
        <output className="empty">No tasks yet. Add one above to get started.</output>
      ) : (
        <ul className="task-list" aria-label="Task list">
          {tasks.map((task) => (
            <li key={task.id} className={`task-item ${task.done ? 'done' : ''}`}>
              <form method="post" action={actionUrl('toggleTask')}>
                <input type="hidden" name="id" value={task.id} />
                <button
                  type="submit"
                  className="ghost"
                  aria-label={
                    task.done ? `Mark "${task.title}" incomplete` : `Mark "${task.title}" complete`
                  }
                >
                  {task.done ? '✓' : '○'}
                </button>
              </form>
              <span className="task-title">{task.title}</span>
              <span className="task-actions">
                <form method="post" action={actionUrl('deleteTask')}>
                  <input type="hidden" name="id" value={task.id} />
                  <button type="submit" className="danger" aria-label={`Delete "${task.title}"`}>
                    ✕
                  </button>
                </form>
              </span>
            </li>
          ))}
        </ul>
      )}
    </main>
  )
}
