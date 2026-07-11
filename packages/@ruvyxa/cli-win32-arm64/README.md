# @ruvyxa/cli-win32-arm64

Prebuilt Ruvyxa native CLI binary for Windows arm64.

This package is installed automatically as an optional dependency of `ruvyxa` on matching platforms.
Application users should install `ruvyxa`, not this package directly.

```bash
npm install ruvyxa
npx ruvyxa doctor
```

The package exists so npm can resolve a platform-specific binary without requiring Rust or Cargo on
user machines.

## Binary Resolution

The main `ruvyxa` package tries this optional package on Windows arm64 after checking for a bundled
native binary. The executable exposed by this package is `ruvyxa.exe`.
