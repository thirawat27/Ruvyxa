<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/adapter-firebase</h1>

<p align="center">
  Full-stack Firebase Hosting adapter for Ruvyxa. It publishes static assets to Firebase's CDN and<br/>
  rewrites dynamic requests to a generated second-generation HTTPS function.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-firebase"><img src="https://img.shields.io/npm/v/@ruvyxa/adapter-firebase?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/adapter-firebase"><img src="https://img.shields.io/node/v/@ruvyxa/adapter-firebase?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```bash
npm run build -- --adapter firebase
firebase deploy --only hosting,functions
```

The build creates `firebase.json` without overwriting an existing file. Firebase project selection
and authentication remain Firebase CLI responsibilities. SSR, SSG, CSR, ISR, PPR, and API routes are
supported; native WebSocket realtime requires a long-lived Node/Bun host instead.
