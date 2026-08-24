import { Image } from '@ruvyxa/react'
import RuvyxaRunner from './components/ruvyxa-runner'

export default function Home() {
  return (
    <main className="page">
      <section className="main" aria-labelledby="home-title">
        <Image className="logo" src="/ruvyxa.png" alt="Ruvyxa logo" width={80} height={80} />
        <h1 className="title" id="home-title">
          Create Ruvyxa App
        </h1>
        <p className="description">
          Edit <code>app/page.tsx</code> to start building your application.
        </p>
        <div className="links">
          <a className="link primary" href="https://github.com/thirawat27/Ruvyxa">
            Docs
          </a>
          <a className="link" href="https://github.com/thirawat27/Ruvyxa/tree/main/examples/demo">
            Examples
          </a>
        </div>
        <RuvyxaRunner />
      </section>
    </main>
  )
}
