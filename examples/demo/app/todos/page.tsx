export default function TodosPage() {
  return (
    <main className="page">
      <p className="eyebrow">Server action</p>
      <h1>Todos</h1>
      <p>
        This demonstrates server actions via <code>action.ts</code>.
      </p>
      <p>
        The form POSTs to <code>/__ruvyxa/action?path=/todos&name=createTodo</code>.
      </p>

      <form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
        <label>
          <span>Title</span>
          <input name="title" defaultValue="Build something great" />
        </label>
        <button type="submit">Create todo</button>
      </form>

      <h2>How it works</h2>
      <pre>{`// app/todos/action.ts
import { action } from "ruvyxa/server"

export const createTodo = action
  .input({ parse: (v) => ({ title: String(v.title).trim() }) })
  .handler(async ({ input, invalidate }) => {
    invalidate("todos")
    return { id: "...", title: input.title, completed: false }
  })`}</pre>
    </main>
  )
}
