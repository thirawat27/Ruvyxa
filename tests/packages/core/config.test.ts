import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { defineConfig, definePlugin, plugin, type RuvyxaConfig } from "../../../packages/@ruvyxa/core/src/config.ts"

describe("config API", () => {
  it("accepts documented middleware configuration", () => {
    const config: RuvyxaConfig = {
      middleware: {
        builtin: {
          timing: true,
          logging: true,
          cors: {
            origins: ["http://localhost:5173"],
            methods: ["GET", "POST"],
            headers: ["Content-Type"],
            credentials: true,
            maxAge: 86400,
          },
          rateLimit: {
            maxRequests: 100,
            windowSecs: 60,
            keyBy: "ip",
          },
          headers: {
            "X-Powered-By": "Ruvyxa",
          },
        },
        plugins: [
          {
            name: "auth-guard",
            path: "plugins/auth-guard.wasm",
            phase: "request",
            hotReload: true,
            routes: ["/api/*"],
            config: { apiKeyHeader: "X-Api-Key" },
            permissions: {
              env: ["AUTH_SECRET"],
              fsRead: ["./content"],
              net: ["api.example.com"],
              timeoutMs: 5000,
              maxMemoryBytes: 67108864,
            },
          },
        ],
      },
      adapterOptions: {
        region: "iad1",
      },
      build: {
        treeShaking: false,
        emitChunkManifest: true,
      },
    }

    const defined = defineConfig(config)

    assert.equal(defined.middleware?.builtin?.timing, true)
    assert.equal(defined.middleware?.plugins?.[0]?.phase, "request")
    assert.equal(defined.adapterOptions?.region, "iad1")
    assert.equal(defined.build?.treeShaking, false)
    assert.equal(defined.build?.emitChunkManifest, true)
  })

  it("accepts plugin helpers and factories", () => {
    const plugin = definePlugin(({ root }) => ({
      name: "replace-label",
      timeoutMs: 1000,
      transform(code, id, ctx) {
        assert.equal(ctx.root, root)
        if (!id.endsWith(".tsx")) return null
        return code.replace("Before", "After")
      },
    }))

    const config: RuvyxaConfig = {
      plugins: [plugin, false, null],
    }

    const defined = defineConfig(config)

    assert.equal(typeof defined.plugins?.[0], "function")
  })

  it("accepts concise plugin shorthand", () => {
    const replaceLabel = plugin("replace-label", (code, id) => {
      if (!id.endsWith(".tsx")) return null
      return code.replace("Before", "After")
    })

    const banner = plugin("banner", {
      enforce: "pre",
      timeoutMs: 1000,
      transform(code) {
        return `/* bundle */\n${code}`
      },
    })

    const config = defineConfig({
      plugins: ["replace-label", replaceLabel, banner],
    })

    assert.equal(config.plugins[0], "replace-label")
    assert.equal(config.plugins[1]?.name, "replace-label")
    assert.equal(config.plugins[2]?.name, "banner")
    assert.equal(config.plugins[2]?.enforce, "pre")
  })
})
