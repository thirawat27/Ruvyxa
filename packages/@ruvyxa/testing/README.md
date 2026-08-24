<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/testing</h1>

<p align="center">
  Small framework-shaped test doubles for Ruvyxa loaders, actions, and caches. They run in Node,<br/>
  Vitest, or Jest without starting a Ruvyxa server.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/testing"><img src="https://img.shields.io/npm/v/@ruvyxa/testing?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/testing"><img src="https://img.shields.io/node/v/@ruvyxa/testing?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
import { mockAction, mockCache, mockLoader } from '@ruvyxa/testing'

const cache = mockCache({ 'posts:list': [] })
const loadPosts = mockLoader(async ({ params }) => ({ params }))
const savePost = mockAction(async ({ input, invalidate }) => {
  invalidate('posts')
  return input
})
```
