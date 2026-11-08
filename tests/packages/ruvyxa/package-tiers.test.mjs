import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import { EXPERIMENTAL_WARNING, tierFailures } from '../../../scripts/check-package-tiers.mjs'

const experimental = (name, extra = {}) => ({
  name,
  manifest: { name, ruvyxa: { tier: 'experimental' }, ...extra },
  readme: `# ${name}\n\n${EXPERIMENTAL_WARNING} No production guarantee.\n`,
})
const stable = (name, dependencies = {}) => ({
  name,
  manifest: { name, dependencies },
  readme: `# ${name}\n`,
})

describe('package tiers', () => {
  it('accepts a marked experimental package and a stable one that does not depend on it', () => {
    assert.deepEqual(tierFailures([experimental('@ruvyxa/auth'), stable('ruvyxa')]), [])
  })

  it('requires the README warning on an experimental package', () => {
    const pkg = experimental('@ruvyxa/auth')
    pkg.readme = '# @ruvyxa/auth\n'
    assert.deepEqual(tierFailures([pkg]), [
      '@ruvyxa/auth is experimental but its README does not open with "> **Experimental.**"',
    ])
  })

  it('accepts a README shaped like the real ones: HTML logo block, h1, then the warning', () => {
    const pkg = experimental('@ruvyxa/auth')
    pkg.readme = [
      '<p align="center">',
      '  <a href="https://example.com"><img src="logo.png" alt="Ruvyxa" /></a>',
      '</p>',
      '',
      '<h1 align="center">@ruvyxa/auth</h1>',
      '',
      `${EXPERIMENTAL_WARNING} No production guarantee.`,
      '',
      '```ts',
      "import '@ruvyxa/auth'",
      '```',
      '',
    ].join('\n')
    assert.deepEqual(tierFailures([pkg]), [])
  })

  it('rejects a README whose warning appears only after a code fence', () => {
    const pkg = experimental('@ruvyxa/auth')
    pkg.readme = `# @ruvyxa/auth\n\n\`\`\`ts\nconst a = 1\n\`\`\`\n\n${EXPERIMENTAL_WARNING} Too late.\n`
    assert.deepEqual(tierFailures([pkg]), [
      '@ruvyxa/auth is experimental but its README does not open with "> **Experimental.**"',
    ])
  })

  it('rejects an unknown tier value', () => {
    const pkg = experimental('@ruvyxa/auth')
    pkg.manifest.ruvyxa.tier = 'beta'
    assert.deepEqual(tierFailures([pkg]), [
      '@ruvyxa/auth declares ruvyxa.tier "beta"; the only tier a package may declare is "experimental"',
    ])
  })

  it('refuses a stable package depending on an experimental one, in any dependency field', () => {
    for (const field of ['dependencies', 'peerDependencies', 'optionalDependencies']) {
      const failures = tierFailures([
        experimental('@ruvyxa/realtime'),
        {
          ...stable('ruvyxa'),
          manifest: { name: 'ruvyxa', [field]: { '@ruvyxa/realtime': 'workspace:^' } },
        },
      ])
      assert.deepEqual(failures, [
        `ruvyxa is Tier 1 but its ${field} names experimental @ruvyxa/realtime`,
      ])
    }
  })

  it('requires the four integration packages to be marked', () => {
    assert.deepEqual(tierFailures([stable('@ruvyxa/database')]), [
      '@ruvyxa/database must declare "ruvyxa": { "tier": "experimental" }',
    ])
  })
})
