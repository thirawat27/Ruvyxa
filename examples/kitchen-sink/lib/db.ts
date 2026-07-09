import 'server-only'

export interface Todo {
  id: string
  title: string
  completed: boolean
}

const todos: Todo[] = [
  { id: '1', title: 'Learn Ruvyxa', completed: true },
  { id: '2', title: 'Build an app', completed: false },
]

export function getTodos(): Todo[] {
  return todos
}

export function addTodo(title: string): Todo {
  const todo: Todo = {
    id: String(todos.length + 1),
    title,
    completed: false,
  }
  todos.push(todo)
  return todo
}
