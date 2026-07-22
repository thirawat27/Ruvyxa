import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { Answer, Image, Picture, Seo } from '../dist/index.js'

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

  it('does not rewrite remote URLs unless an explicit loader controls them', () => {
    const remote = renderToStaticMarkup(
      React.createElement(Image, {
        src: 'https://cdn.example.com/hero.jpg',
        alt: '',
        width: 10,
        height: 10,
      }),
    )
    const loaded = renderToStaticMarkup(
      React.createElement(Image, {
        src: 'https://origin.example.com/hero.jpg',
        alt: '',
        width: 320,
        height: 180,
        quality: 75,
        loader: ({ src, width, quality }) =>
          `https://cdn.example.com/image?src=${encodeURIComponent(src)}&w=${width}&q=${quality}`,
      }),
    )
    assert.match(remote, /src="https:\/\/cdn\.example\.com\/hero\.jpg"/)
    assert.match(
      loaded,
      /src="https:\/\/cdn\.example\.com\/image\?src=https%3A%2F%2Forigin\.example\.com%2Fhero\.jpg&amp;w=320&amp;q=75"/,
    )
  })

  it('supports fill layouts, author-provided srcsets, and native picture sources', () => {
    const filled = renderToStaticMarkup(
      React.createElement(Image, {
        src: '/hero.jpg',
        alt: '',
        fill: true,
        srcSet: '/hero.jpg 1x, /hero@2x.jpg 2x',
        sizes: '100vw',
        style: { objectFit: 'cover' },
      }),
    )
    const picture = renderToStaticMarkup(
      React.createElement(Picture, {
        src: '/hero.jpg',
        alt: 'Hero',
        width: 1200,
        height: 630,
        sources: [
          { media: '(max-width: 600px)', srcSet: '/hero-mobile.png' },
          { media: '(min-width: 601px)', srcSet: '/hero-desktop.jpg' },
        ],
      }),
    )
    assert.match(filled, /src="\/hero\.webp"/)
    assert.match(filled, /srcSet="\/hero\.webp 1x, \/hero@2x\.webp 2x"/)
    assert.match(filled, /position:absolute/)
    assert.match(filled, /object-fit:cover/)
    assert.match(picture, /<picture>/)
    assert.match(picture, /srcSet="\/hero-mobile\.webp"/)
    assert.match(picture, /srcSet="\/hero-desktop\.webp"/)
    assert.match(picture, /type="image\/webp"/)
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

  it('derives article and breadcrumb JSON-LD from explicit page facts', () => {
    const html = renderToStaticMarkup(
      React.createElement(Seo, {
        title: 'How Ruvyxa renders',
        description: 'An evidence-backed rendering guide.',
        canonical: 'https://example.com/guides/rendering',
        image: 'https://example.com/rendering.png',
        article: {
          type: 'BlogPosting',
          publishedAt: '2026-07-22',
          updatedAt: '2026-07-23T10:30:00+07:00',
          authors: [{ name: 'Ada', url: 'https://example.com/authors/ada' }],
          section: 'Guides',
          tags: ['SSR', 'React'],
        },
        breadcrumbs: [
          { name: 'Home', url: 'https://example.com/' },
          { name: 'Guides', url: 'https://example.com/guides' },
          { name: 'Rendering', url: 'https://example.com/guides/rendering' },
        ],
        jsonLd: { '@context': 'https://schema.org', '@type': 'WebSite', name: '</script>' },
      }),
    )
    const script = html.match(/<script type="application\/ld\+json">([^]*?)<\/script>/)?.[1]
    assert.ok(script)
    const structuredData = JSON.parse(script)
    assert.equal(structuredData[0]['@type'], 'BlogPosting')
    assert.equal(structuredData[0].mainEntityOfPage['@id'], 'https://example.com/guides/rendering')
    assert.deepEqual(structuredData[0].author, [
      { '@type': 'Person', name: 'Ada', url: 'https://example.com/authors/ada' },
    ])
    assert.equal(structuredData[1]['@type'], 'BreadcrumbList')
    assert.equal(structuredData[1].itemListElement[2].position, 3)
    assert.equal(structuredData[2]['@type'], 'WebSite')
    assert.doesNotMatch(html, /<\/script><\/script>/)
  })

  it('renders visible answer text, citations, and Question/Answer microdata', () => {
    const html = renderToStaticMarkup(
      React.createElement(
        Answer,
        {
          id: 'rendering-answer',
          question: 'Does Ruvyxa render on the server?',
          sourcesLabel: 'References',
          sources: [{ name: 'Rendering guide', url: 'https://example.com/docs/rendering' }],
        },
        React.createElement('p', null, 'Yes. Pages render on the server by default.'),
      ),
    )
    assert.match(html, /aria-labelledby="rendering-answer"/)
    assert.match(html, /itemType="https:\/\/schema\.org\/Question"/)
    assert.match(html, /itemProp="acceptedAnswer"/)
    assert.match(html, /Pages render on the server by default/)
    assert.match(html, /itemProp="citation"/)
    assert.match(html, />References</)
  })
})
