# Styling, SCSS, and CSS Modules

> 🟢 **Beginner friendly** · ⏱️ ~5 min read
>
> **You'll learn:** global stylesheets, Sass/SCSS, and per-component CSS Modules — all with hot
> reload and zero extra configuration.

Ruvyxa handles global CSS, SCSS/Sass, and locally scoped CSS Modules in the normal module graph.
Imported styles can live anywhere inside the project.

## Global CSS and SCSS

Import a global stylesheet from a layout or component:

```tsx
import './globals.scss'
```

Both `.scss` and indented `.sass` syntax are compiled automatically. Sass partials referenced by
`@use`, `@forward`, or `@import` are included in compilation and watched during development.

Unimported global styles belong in `css.entries`:

```ts
import { config } from 'ruvyxa/config'

export default config({
  css: { entries: ['styles/theme.scss'] },
})
```

## CSS Modules

Name a file `.module.css`, `.module.scss`, or `.module.sass` and import its default export:

```scss
// app/card.module.scss
$accent: #7c3aed;

.card {
  border: 1px solid $accent;

  .title {
    color: $accent;
  }
}
```

```tsx
import styles from './card.module.scss'

export function Card() {
  return (
    <article className={styles.card}>
      <h2 className={styles.title}>Scoped styles</h2>
    </article>
  )
}
```

The default export maps each local class to a deterministic class name derived from the
project-relative file path and original class. The emitted CSS uses the same names, preventing
collisions across components while keeping builds reproducible. Production minification and dev HMR
use the same module mapping.

CSS Modules scope local class selectors. Plain CSS nesting is scoped in the same way as top-level
selectors. `:global(.name)` intentionally leaves that selector global, while local
`composes: other;` adds the composed class to the exported map and removes the declaration from
emitted CSS. Cross-file composition (`composes: other from './other.module.css'`) is not supported
by the built-in transformer and should be replaced with an explicit class composition in code.

## TypeScript

The `ruvyxa` package owns the ambient declarations. Global style imports are side-effect modules;
`.module.*` imports provide `Readonly<Record<string, string>>`. No app-local `css.d.ts` is needed.

## Tailwind and LESS

Tailwind remains auto-detected from `@import "tailwindcss"`. LESS is not compiled by the built-in
pipeline and produces a diagnostic; use a transform plugin if a project requires LESS.
