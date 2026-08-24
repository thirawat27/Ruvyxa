import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { describe, it } from 'node:test'

const policy = readFileSync(new URL('../../../SECURITY.md', import.meta.url), 'utf8')
const workflow = readFileSync(
  new URL('../../../.github/workflows/security.yml', import.meta.url),
  'utf8',
)
const workspacePackage = JSON.parse(
  readFileSync(new URL('../../../package.json', import.meta.url), 'utf8'),
)

describe('dependency security workflow', () => {
  it('audits both production lockfiles with read-only repository access', () => {
    assert.match(workflow, /cargo install cargo-audit --version 0\.22\.2 --locked/)
    assert.match(workflow, /cargo audit --file Cargo\.lock/)
    assert.match(workflow, /pnpm audit --prod --audit-level low/)
    assert.match(workflow, /permissions:\s+contents: read/)
    assert.doesNotMatch(workflow, /(?:checks|issues|pull-requests): write/)
    assert.match(workflow, /persist-credentials: false/)
  })

  it('uses a package manager compatible with the exact minimum Node runtime', () => {
    assert.equal(workspacePackage.engines.node, '>=24.19.0')
    // The pin itself is asserted once, in native-platform.test.mjs. Repeating the
    // literal here made a pnpm bump a two-file edit, and this file is the one that
    // got remembered while the other failed CI.
    assert.match(workspacePackage.packageManager, /^pnpm@\d+\.\d+\.\d+$/)
    assert.match(workflow, /node-version: 24\.19\.0/)
    // Pinned to a commit, with the readable major in the trailing comment.
    // `crates/ruvyxa_cli/tests/ci_workflows.rs` holds the pin *format* for
    // every action in every workflow; what this line still owns is which
    // major this workflow is meant to be on.
    assert.match(workflow, /uses: pnpm\/action-setup@[0-9a-f]{40} # v6/)
  })

  it('runs on a schedule and dependency changes', () => {
    assert.match(workflow, /schedule:/)
    assert.match(workflow, /push:\s+paths:/)
    assert.match(workflow, /pull_request:\s+paths:/)
    assert.match(workflow, /workflow_dispatch:/)
    assert.match(workflow, /Cargo\.lock/)
    assert.match(workflow, /pnpm-lock\.yaml/)
    assert.match(policy, /Scheduled and change-triggered RustSec plus pnpm production dependency/)
  })

  it('documents the plugin trust boundary without claiming a sandbox', () => {
    assert.match(policy, /TypeScript plugins run as trusted application code/)
    assert.match(policy, /security\.pluginLimit[\s\S]{0,160}resource limit/)
    assert.doesNotMatch(policy, /Wasm plugin|plugin sandboxing/)
  })
})
