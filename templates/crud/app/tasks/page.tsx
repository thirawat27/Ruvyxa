import { getTasks } from './server'

/**
 * Tasks page — a server component that reads tasks from the data loader
 * and renders a form with server actions for mutations.
 */
export default async function TasksPage() {
  const tasks = await getTasks()

  return (
    <main>
      <h1>Tasks</h1>
      <p>Manage your task list. Changes are handled by server actions.</p>

      <form
        method="post"
        action="/__ruvyxa/action?path=/tasks&name=createTask"
        aria-label="Add a new task"
      >
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
        <p className="empty" role="status">
          No tasks yet. Add one above to get started.
        </p>
      ) : (
        <ul className="task-list" aria-label="Task list">
          {tasks.map((task) => (
            <li key={task.id} className={`task-item ${task.done ? 'done' : ''}`}>
              <form
                method="post"
                action="/__ruvyxa/action?path=/tasks&name=toggleTask"
                aria-label={`Toggle "${task.title}"`}
              >
                <input type="hidden" name="id" value={task.id} />
                <button
                  type="submit"
                  className="ghost"
                  aria-label={task.done ? 'Mark as incomplete' : 'Mark as complete'}
                >
                  {task.done ? '✓' : '○'}
                </button>
              </form>
              <span className="task-title">{task.title}</span>
              <span className="task-actions">
                <form
                  method="post"
                  action="/__ruvyxa/action?path=/tasks&name=deleteTask"
                  aria-label={`Delete "${task.title}"`}
                >
                  <input type="hidden" name="id" value={task.id} />
                  <button type="submit" className="danger" aria-label="Delete task">
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
