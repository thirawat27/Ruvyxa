<p align="center">
  <a href="https://github.com/thirawat27/ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/cli-win32-arm64</h1>

<p align="center">
  Prebuilt Ruvyxa CLI binary for Windows arm64.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/cli-win32-arm64"><img src="https://img.shields.io/npm/v/@ruvyxa/cli-win32-arm64?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/cli-win32-arm64"><img src="https://img.shields.io/node/v/@ruvyxa/cli-win32-arm64?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

This package is installed automatically as an optional dependency of `ruvyxa` on matching platforms.
Application users should install `ruvyxa`, not this package directly.

```bash
npm create ruvyxa@latest my-app
cd my-app
npm install
npm run doctor
```

The package exists so npm can resolve a platform-specific binary without requiring Rust or Cargo on
user machines.

## Binary Resolution

The main `ruvyxa` package tries this optional package on Windows arm64 after checking for a bundled
native binary. The executable exposed by this package is `ruvyxa.exe`.
