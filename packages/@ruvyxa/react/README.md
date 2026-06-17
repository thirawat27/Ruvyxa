# @ruvyxa/react

React integration package for Ruvyxa apps.

## Install

```bash
npm install @ruvyxa/react react react-dom
```

React and ReactDOM are peer dependencies. Most app users do not import this package directly; the main `ruvyxa` runtime uses React SSR and route-level client bundling internally.

## When to Use Directly

Use this package for React-specific integration work, framework experiments, or future adapter/runtime composition. For ordinary apps, import public APIs from `ruvyxa/config` and `ruvyxa/server`.
