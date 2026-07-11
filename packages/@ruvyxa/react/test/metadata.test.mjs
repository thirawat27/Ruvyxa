import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { Image, Seo } from '../dist/index.js'

describe('@ruvyxa/react image and SEO primitives', () => {
  it('renders modern image sources with an intrinsic lazy fallback', () => {
    const html = renderToStaticMarkup(
      React.createElement(Image, {
        src: '/hero.png',
        alt: 'Ruvyxa hero',
        width: 1200,
        height: 630,
      }),
    )
    assert.match(html, /srcSet="\/hero\.png\.avif"/)
    assert.match(html, /srcSet="\/hero\.png\.webp"/)
    assert.match(html, /width="1200"/)
    assert.match(html, /height="630"/)
    assert.match(html, /loading="lazy"/)
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
