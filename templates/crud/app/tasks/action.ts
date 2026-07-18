import { action } from 'ruvyxa/server'
import { addTask, toggleTaskById, deleteTaskById } from './server'

/**
 * Server action to create a new task.
 * Validates that the title is non-empty and within length limits.
 */
const taskTitle = {
  parse(value: unknown) {
    if (!value || typeof value !== 'object' || !('title' in value)) {
      throw new Error('Task title is required.')
    }
    const title = String(value.title).trim()
    if (!title) throw new Error('Task title is required.')
    if (title.length > 200) throw new Error('Task title must be 200 characters or fewer.')
    return { title }
  },
}

const taskId = {
  parse(value: unknown) {
    if (!value || typeof value !== 'object' || !('id' in value)) {
      throw new Error('Task ID is required.')
    }
    const id = String(value.id).trim()
    if (!id) throw new Error('Task ID is required.')
    return { id }
  },
}

export const createTask = action.input(taskTitle).handler(async ({ input, invalidate }) => {
  const task = addTask(input.title)
  invalidate('tasks')
  return { ok: true, task }
})

/**
 * Server action to toggle a task's done state.
 */
export const toggleTask = action.input(taskId).handler(async ({ input, invalidate }) => {
  const success = toggleTaskById(input.id)
  if (!success) {
    return { error: 'Task not found.' }
  }
  invalidate('tasks')
  return { ok: true }
})

/**
 * Server action to delete a task.
 */
export const deleteTask = action.input(taskId).handler(async ({ input, invalidate }) => {
  const success = deleteTaskById(input.id)
  if (!success) {
    return { error: 'Task not found.' }
  }
  invalidate('tasks')
  return { ok: true }
})
