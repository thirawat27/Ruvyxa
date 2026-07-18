export const meta = {
  title: 'About - My Ruvyxa Blog',
  description: 'Learn more about this blog and its author.',
}

export default function About() {
  return (
    <section aria-labelledby="about-title">
      <h1 id="about-title">About</h1>
      <p>
        This blog is built with Ruvyxa, a modern full-stack web framework powered by Rust and React.
        It serves as a starting point for content-focused websites.
      </p>
      <p>
        Ruvyxa combines server-side rendering, static generation, and client-side interactivity into
        a cohesive developer experience with file-based routing.
      </p>
      <h2>About the Author</h2>
      <p>
        Replace this section with your own bio. Tell your readers who you are, what you write about,
        and how they can get in touch.
      </p>
    </section>
  )
}
