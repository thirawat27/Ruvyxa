# Dev/Production Parity

Ruvyxa keeps route semantics aligned between `dev` and `start`.

Run the parity check:

```bash
ruvyxa test:parity
```

The command builds production output, discovers routes from `app/`, discovers routes again from `.ruvyxa/server/app`, and compares:

- route kind and path
- page or route file
- layout chain
- server modules
- client modules
- runtime target

Example output:

```txt
PASS Page / dev/prod match
PASS Page /todos dev/prod match
PASS Api /api/health dev/prod match
Parity passed for 5 routes
```

Use this after changing routing, layouts, server modules, actions, or build output.
