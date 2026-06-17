# Ruvyxa Example App Agent Guide

This example app is used to verify the Ruvyxa framework runtime.

Keep changes small and representative of real user apps. If you add a feature here to test framework behavior, update `templates/minimal/` when that feature should be available to new apps.

## Checks

```bash
cargo run -p ruvyxa_cli -- analyze --root .
cargo run -p ruvyxa_cli -- build --root .
```
