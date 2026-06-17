# @ruvyxa/cli-linux-arm64

Prebuilt Ruvyxa native CLI binary for Linux arm64.

This package is installed automatically as an optional dependency of `ruvyxa` on matching platforms. Application users should install `ruvyxa`, not this package directly.

```bash
npm install ruvyxa
npx ruvyxa doctor
```

The package exists so npm can resolve a platform-specific binary without requiring Rust or Cargo on user machines.
