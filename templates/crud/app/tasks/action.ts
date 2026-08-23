import { action } from 'ruvyxa/server'

import { addTask, deleteTaskById, toggleTaskById } from './server'

/**
 * Server actions for the task list.
 *
 * `action.input(schema)` takes anything with a synchronous `parse(value)`, so
 * the two schemas below can be replaced wholesale by a schema library without
 * touching a handler. Validation runs on the server either way: the browser
 * sees a form, and a form is a suggestion.
 *
 * A handler that throws answers with the error; one that returns `{ error }`
 * answers with a result the caller can render. Both are used below — a missing
 * field is a malformed request, while a task that no longer exists is a normal
 * outcome of two tabs racing.
 */

/** Read one required, trimmed field out of an unknown payload. */
function requiredField(value: unknown, field: string, maxLength: number): string {
  if (!value || typeof value !== 'object' || !(field in value)) {
    throw new Error(`Field "${field}" is required.`)
  }
  const text = String((value as Record<string, unknown>)[field]).trim()
  if (!text) throw new Error(`Field "${field}" is required.`)
  if (text.length > maxLength) {
    throw new Error(`Field "${field}" must be ${maxLength} characters or fewer.`)
  }
  return text
}

const taskTitle = {
  parse: (value: unknown) => ({ title: requiredField(value, 'title', 200) }),
}

const taskId = {
  parse: (value: unknown) => ({ id: requiredField(value, 'id', 64) }),
}

/** Create a task and drop the cached list that no longer describes reality. */
export const createTask = action.input(taskTitle).handler(async ({ input, invalidate }) => {
  const task = addTask(input.title)
  invalidate('tasks')
  return { ok: true, task }
})

/** Flip a task between done and not done. */
export const toggleTask = action.input(taskId).handler(async ({ input, invalidate }) => {
  if (!toggleTaskById(input.id)) return { error: 'Task not found.' }
  invalidate('tasks')
  return { ok: true }
})

/** Remove a task. */
export const deleteTask = action.input(taskId).handler(async ({ input, invalidate }) => {
  if (!deleteTaskById(input.id)) return { error: 'Task not found.' }
  invalidate('tasks')
  return { ok: true }
})
