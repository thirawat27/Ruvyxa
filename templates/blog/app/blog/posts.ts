export interface Post {
  slug: string
  title: string
  date: string
  excerpt: string
  content: string[]
}

// All posts, newest first. Replace this array with a database, a CMS, or
// Markdown files when the blog outgrows it — the pages read it through the two
// helpers below, so nothing else has to change.
export const posts: Post[] = [
  {
    slug: 'getting-started',
    title: 'Getting Started with Ruvyxa',
    date: '2026-08-18',
    excerpt: 'Create a project, find your way around it, and add your first route.',
    content: [
      'Ruvyxa is a full-stack web framework with a Rust compiler and server, and a TypeScript runtime and API. Routing is file-based: a folder under app/ with a page.tsx becomes a route, and the folder name is the URL.',
      'This post lives in app/blog/posts.ts and is rendered by app/blog/[slug]/page.tsx. The [slug] folder is a dynamic segment, and the page tells the build which ones to pre-render by exporting getStaticParams.',
    ],
  },
  {
    slug: 'rendering-strategies',
    title: 'Choosing a Rendering Strategy',
    date: '2026-08-11',
    excerpt: 'SSG, ISR, SSR, CSR, and PPR — what each one decides.',
    content: [
      'A strategy answers one question: when is this page HTML produced? Ruvyxa reads the answer from the route own exports, so the page and its strategy cannot disagree.',
      'A static route is SSG, built once. Export revalidate = 60 for ISR. The default is SSR, rendered per request. A page marked use client is CSR. Export ppr = true with a Suspense boundary for a static shell with a streaming slot.',
      'Every post in this blog is SSG: nothing about a published post changes between two requests, so nothing should be recomputed between them.',
    ],
  },
  {
    slug: 'styling',
    title: 'Styling a Ruvyxa App',
    date: '2026-08-04',
    excerpt: 'Global CSS, CSS Modules, and Sass, without a configuration step.',
    content: [
      'Import a stylesheet from any module and it is bundled: app/globals.css is imported by app/layout.tsx, which is why it applies everywhere.',
      'A file named *.module.css gives a component a locally scoped class map. Sass works by importing .scss or .sass directly. For a global stylesheet no module imports, list it under css.entries in ruvyxa.config.ts.',
    ],
  },
  {
    slug: 'deploying',
    title: 'Deploying Your Blog',
    date: '2026-07-28',
    excerpt: 'One adapter flag, and what the build publishes alongside the pages.',
    content: [
      'A blog of static routes deploys anywhere that serves files. Install an adapter and name it: ruvyxa build --adapter static. Adapters exist for Node, Bun, Deno, Vercel, Netlify, Cloudflare, Railway, Render, Firebase, AWS, and static hosting.',
      'If a route needs something the target cannot do, the build says so and stops instead of deploying something that fails at request time. Set site.url in ruvyxa.config.ts first so the build can publish a sitemap alongside robots.txt.',
    ],
  },
]

export function findPost(slug: string): Post | undefined {
  return posts.find((post) => post.slug === slug)
}

// `timeZone: 'UTC'` matters. `new Date('2026-08-18')` is midnight UTC, so
// formatting it in the machine's own zone gives the 17th anywhere west of
// Greenwich — the server would render one date and the browser another.
export function formatDate(iso: string): string {
  return new Date(iso).toLocaleDateString('en-US', {
    timeZone: 'UTC',
    year: 'numeric',
    month: 'long',
    day: 'numeric',
  })
}
