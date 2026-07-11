import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object' || !('title' in value)) {
        throw new Error('Todo title is required')
      }
      const title = String(value.title).trim()
      if (!title) throw new Error('Todo title is required')
      return { title }
    },
  })
  .handler(async ({ input, invalidate }) => {
    invalidate('todos')
    return {
      id: crypto.randomUUID(),
      title: input.title,
      completed: false,
    }
  })
