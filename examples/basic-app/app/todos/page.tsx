export default function TodosPage() {
  return (
    <main className="page">
      <p className="eyebrow">Server action</p>
      <h1>Todos</h1>
      <p>
        This route exposes <code>createTodo</code> from <code>action.ts</code>. Submit JSON to{" "}
        <code>/__ruvyxa/action?path=/todos&amp;name=createTodo</code>.
      </p>
      <form method="post" action="/__ruvyxa/action?path=/todos&name=createTodo">
        <label>
          <span>Title</span>
          <input name="title" defaultValue="Ship Ruvyxa" />
        </label>
        <button type="submit">Create todo</button>
      </form>
    </main>
  )
}
