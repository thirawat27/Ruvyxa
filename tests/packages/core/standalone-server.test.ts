import assert from 'node:assert/strict'
import { execFileSync } from 'node:child_process'
import { mkdtempSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import path from 'node:path'
import { describe, it } from 'node:test'

import { standaloneServerSource } from '../../../packages/@ruvyxa/core/dist/standalone-server.js'

const generated = standaloneServerSource({ runtimePolicy: {} })

/**
 * The node, bun, deno, aws, railway, and render adapters all deploy the source
 * this module returns. It is assembled as a template string, so nothing in the
 * normal build checks it: `tsc` validates the *template*, not the JavaScript
 * that comes out of it, and the first execution happens on the user's host
 * after a deploy. These tests are the only place the emitted program is checked
 * before it ships.
 */
describe('generated standalone server', () => {
  /**
   * A single unescaped backtick anywhere in the template — including inside a
   * comment — closes the string early and emits a file that cannot parse. That
   * failure surfaces at deploy time on the user's machine, not here, so the
   * emitted program is parsed as the program it will actually be.
   */
  it('emits syntactically valid JavaScript', () => {
    const directory = mkdtempSync(path.join(tmpdir(), 'ruvyxa-standalone-'))
    const file = path.join(directory, 'index.mjs')
    writeFileSync(file, generated, 'utf8')
    execFileSync(process.execPath, ['--check', file], { stdio: 'pipe' })
  })

  /**
   * Every container platform stops a deploy by sending SIGTERM and killing the
   * process shortly after. Node's default action is to exit immediately, which
   * drops every response still being written — so a rolling deploy shows users
   * connection resets rather than being invisible.
   */
  it('drains in-flight requests on a shutdown signal', () => {
    assert.match(generated, /process\.on\(signal/, 'must install signal handlers')
    assert.ok(generated.includes("'SIGTERM'"), 'SIGTERM is what orchestrators send')
    assert.ok(generated.includes("'SIGINT'"), 'SIGINT is what a local operator sends')
    assert.ok(
      generated.includes('server.close('),
      'must stop accepting and wait for in-flight work',
    )
    assert.ok(
      generated.includes('closeIdleConnections'),
      'idle keep-alive sockets would otherwise hold the drain open for a full keep-alive window',
    )
  })

  /**
   * The 502 this prevents is the classic one: the proxy keeps a pooled socket
   * it believes is alive, the origin has already started closing it, and the
   * request that lands on it fails. It appears only under load, only in
   * production, and only intermittently.
   */
  it('keeps connections alive longer than a load balancer will', () => {
    const keepAlive = /RUVYXA_KEEP_ALIVE_TIMEOUT', ([\d_]+)\)/.exec(generated)
    assert.ok(keepAlive, 'keep-alive timeout must be set explicitly, not left at the Node default')
    const milliseconds = Number(keepAlive[1].replaceAll('_', ''))
    // AWS ALB idles at 60s and is the tightest of the common managed proxies;
    // anything at or below that lets the origin retire the connection first.
    assert.ok(
      milliseconds > 60_000,
      `keep-alive must exceed a 60s proxy idle window, got ${milliseconds}ms`,
    )
    assert.ok(
      generated.includes('server.headersTimeout'),
      'headersTimeout must be raised with it or Node times out a connection it would have kept',
    )
  })

  /**
   * A rejection thrown outside a request's own try/catch terminates the process
   * by default, taking every concurrent request with it. One bad route must not
   * be able to do that. An uncaught exception is treated differently on purpose:
   * the process state is no longer trustworthy, so it drains and exits non-zero
   * for the supervisor to replace.
   */
  it('survives an unhandled rejection but replaces itself after an uncaught exception', () => {
    assert.ok(generated.includes("process.on('unhandledRejection'"), 'must be handled')
    assert.ok(generated.includes("process.on('uncaughtException'"), 'must be handled')
    const uncaught = generated.slice(generated.indexOf("process.on('uncaughtException'"))
    assert.match(
      uncaught,
      /shutdown\('uncaught exception', 1\)/,
      'an uncaught exception must drain and exit non-zero, not keep serving',
    )
  })

  /**
   * Both pipes commit their status and headers before the body flows, so a
   * later failure can only end the connection. Left unhandled, the stream's
   * `error` event is fatal to the whole process — an aborted download would
   * take out every other request being served.
   */
  it('contains stream failures and client disconnects', () => {
    assert.ok(
      generated.includes("file.on('error'"),
      'a static read that fails mid-response must not be fatal',
    )
    assert.ok(
      generated.includes("body.on('error'"),
      'a response stream that fails mid-response must not be fatal',
    )
    assert.ok(
      generated.includes("res.on('close', () => file.destroy())"),
      'a client that leaves must stop the file read',
    )
    assert.ok(
      generated.includes("res.on('close', () => body.destroy())"),
      'a client that leaves must stop the render still producing for it',
    )
  })
})
