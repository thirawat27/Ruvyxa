// Ruvyxa runs this plugin chain over every global stylesheet, on one code path
// for `ruvyxa dev` and `ruvyxa build`, so the two produce the same CSS.
// Tailwind CSS v4 needs nothing framework-specific beyond this file and the
// `@import 'tailwindcss'` at the top of `app/globals.css`.
export default {
  plugins: {
    '@tailwindcss/postcss': {},
  },
}
