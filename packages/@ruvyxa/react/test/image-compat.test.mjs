import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import React from 'react'
import { renderToStaticMarkup } from 'react-dom/server'

import { Image, Picture } from '../dist/index.js'

describe('@ruvyxa/react image URL compatibility', () => {
  it('keeps protocol-relative CDN URLs unchanged', () => {
    const image = renderToStaticMarkup(
      React.createElement(Image, {
        src: '//cdn.example.com/hero.jpg',
        srcSet: '//cdn.example.com/hero-small.jpg 480w, //cdn.example.com/hero.jpg 960w',
        alt: 'CDN image',
        width: 960,
        height: 540,
      }),
    )
    const picture = renderToStaticMarkup(
      React.createElement(Picture, {
        sources: [{ srcSet: '//cdn.example.com/wide.png 1x', type: 'image/png' }],
        src: '//cdn.example.com/fallback.jpg',
        alt: 'CDN picture',
        width: 960,
        height: 540,
      }),
    )

    assert.match(image, /src="\/\/cdn\.example\.com\/hero\.jpg"/)
    assert.match(image, /srcSet="\/\/cdn\.example\.com\/hero-small\.jpg 480w/)
    assert.doesNotMatch(image, /\.webp/)
    assert.match(picture, /srcSet="\/\/cdn\.example\.com\/wide\.png 1x"/)
    assert.match(picture, /type="image\/png"/)
    assert.doesNotMatch(picture, /\.webp/)
  })
})
