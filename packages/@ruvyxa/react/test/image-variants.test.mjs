import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { describe, it } from 'node:test'

import { DEFAULT_DEVICE_WIDTHS, Image } from '../dist/image.js'

// Render the component to a plain element tree. The compiled JSX targets the
// automatic runtime, so calling the function directly returns the <img>
// descriptor without a DOM.
function renderImage(props) {
  const element = Image(props)
  return element.props
}

describe('Image responsive srcset', () => {
  it('builds a srcset from device widths below the intrinsic width', () => {
    const { srcSet } = renderImage({
      src: '/hero.jpg',
      alt: '',
      width: 1000,
      height: 500,
      sizes: '100vw',
    })

    // Breakpoints under 1000 (640, 750, 828) as variants, then the full-size
    // WebP at the intrinsic width. Nothing at or above 1000 (would upscale).
    assert.equal(
      srcSet,
      '/hero-640w.webp 640w, /hero-750w.webp 750w, /hero-828w.webp 828w, /hero.webp 1000w',
    )
  })

  it('emits no auto srcset without a sizes hint', () => {
    const { srcSet } = renderImage({ src: '/hero.jpg', alt: '', width: 1000, height: 500 })
    assert.equal(srcSet, undefined)
  })

  it('leaves an explicit srcSet under author control (rewriting local URLs)', () => {
    const { srcSet } = renderImage({
      src: '/hero.jpg',
      alt: '',
      width: 1000,
      height: 500,
      sizes: '100vw',
      srcSet: '/a.png 1x, /b.png 2x',
    })
    assert.equal(srcSet, '/a.webp 1x, /b.webp 2x')
  })

  it('does not fabricate variants for a remote or already-optimized source', () => {
    assert.equal(
      renderImage({
        src: 'https://cdn.example/x.jpg',
        alt: '',
        width: 1000,
        height: 500,
        sizes: '100vw',
      }).srcSet,
      undefined,
    )
    assert.equal(
      renderImage({ src: '/logo.svg', alt: '', width: 1000, height: 500, sizes: '100vw' }).srcSet,
      undefined,
    )
  })

  it('skips auto srcset when a loader or unoptimized owns the URL', () => {
    assert.equal(
      renderImage({
        src: '/hero.jpg',
        alt: '',
        width: 1000,
        height: 500,
        sizes: '100vw',
        unoptimized: true,
      }).srcSet,
      undefined,
    )
  })
})

describe('device width list parity with the Rust optimizer', () => {
  it('matches DEFAULT_VARIANT_WIDTHS in image_optimizer.rs', async () => {
    const rustSource = await readFile(
      new URL('../../../../crates/ruvyxa_cli/src/image_optimizer.rs', import.meta.url),
      'utf8',
    )
    const match = rustSource.match(/DEFAULT_VARIANT_WIDTHS: \[u32; \d+\] = \[([^\]]+)\]/)
    assert.ok(match, 'could not find DEFAULT_VARIANT_WIDTHS in image_optimizer.rs')
    const rustWidths = match[1]
      .split(',')
      .map((value) => Number(value.trim()))
      .filter((value) => Number.isFinite(value))
    assert.deepEqual(rustWidths, [...DEFAULT_DEVICE_WIDTHS])
  })
})
