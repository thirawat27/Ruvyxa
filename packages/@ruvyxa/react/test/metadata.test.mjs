import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { Image, Seo } from '../dist/index.js'

describe('@ruvyxa/react image and SEO primitives', () => {
  it('renders the single WebP build output with intrinsic lazy loading', () => {
    const html = renderToStaticMarkup(
      React.createElement(Image, {
        src: '/hero.png',
        alt: 'Ruvyxa hero',
        width: 1200,
        height: 630,
      }),
    )
    assert.match(html, /src="\/hero\.webp"/)
    assert.doesNotMatch(html, /<picture/)
    assert.match(html, /width="1200"/)
    assert.match(html, /height="630"/)
    assert.match(html, /loading="lazy"/)
  })

  it('preserves query strings and supports explicitly unoptimized sources', () => {
    const optimized = renderToStaticMarkup(
      React.createElement(Image, {
        src: '/hero.jpg?v=1#preview',
        alt: '',
        width: 10,
        height: 10,
      }),
    )
    const remote = renderToStaticMarkup(
      React.createElement(Image, {
        src: 'https://cdn.example.com/hero.jpg',
        alt: '',
        width: 10,
        height: 10,
        unoptimized: true,
      }),
    )
    assert.match(optimized, /src="\/hero\.webp\?v=1#preview"/)
    assert.match(remote, /src="https:\/\/cdn\.example\.com\/hero\.jpg"/)
  })

  it('renders canonical, social, robots, and safe JSON-LD metadata', () => {
    const html = renderToStaticMarkup(
      React.createElement(Seo, {
        title: 'Ruvyxa',
        description: 'Fast framework',
        canonical: 'https://example.com/',
        image: 'https://example.com/hero.png',
        jsonLd: { '@context': 'https://schema.org', name: '</script>' },
      }),
    )
    assert.match(html, /rel="canonical"/)
    assert.match(html, /property="og:image"/)
    assert.match(html, /name="twitter:card"/)
    assert.match(html, /index, follow/)
    assert.doesNotMatch(html, /<\/script><\/script>/)
  })
})
