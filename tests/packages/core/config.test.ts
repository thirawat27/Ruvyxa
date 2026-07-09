import { describe, it } from "node:test"
import assert from "node:assert/strict"

import { defineConfig, type RuvyxaConfig } from "../../../packages/@ruvyxa/core/src/config.ts"

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
})
