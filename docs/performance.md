# Performance

Ruvyxa ships a small benchmark suite through the CLI:

```bash
ruvyxa bench --root examples/basic-app --samples 3
```

It reports:

- `route-discovery`: route graph creation from `app/`
- `analyze-validation`: route graph plus server/client boundary validation
- `production-build`: `.ruvyxa` server, assets, manifest, and optimized client bundle output

Use JSON output in CI or perf dashboards:

```bash
ruvyxa bench --root examples/basic-app --samples 5 --json
```

Production builds emit BLAKE3-hashed route-level browser chunks under `.ruvyxa/client`:

```txt
.ruvyxa/client/
├─ <hash>.js
└─ manifest.json
```

The client manifest records the route path, hashed file, byte size, minify flag, tree-shaking flag, and chunk strategy. `ruvyxa start` reads that manifest and injects the prebuilt route bundle instead of using the dev-time bundle endpoint. The companion `build.json` records the hash algorithm and security defaults used for the build.
