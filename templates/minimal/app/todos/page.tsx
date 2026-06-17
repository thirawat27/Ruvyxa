export default function TodosPage() {
  return (
    <main className="page">
      <h1>Todos</h1>
      <p>Server actions live in action.ts next to the page.</p>
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
