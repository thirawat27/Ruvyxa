import type { ImgHTMLAttributes, ReactElement } from 'react'

export interface ImageProps extends Omit<
  ImgHTMLAttributes<HTMLImageElement>,
  'src' | 'alt' | 'width' | 'height' | 'loading' | 'fetchPriority'
> {
  /** Public image URL. Local PNG/JPEG URLs are rewritten to their build-time WebP output. */
  src: string
  /** Required accessible alternative text. Use an empty string for decorative images. */
  alt: string
  /** Intrinsic width prevents cumulative layout shift. */
  width: number
  /** Intrinsic height prevents cumulative layout shift. */
  height: number
  /** Keep `src` unchanged for remote URLs or assets managed outside Ruvyxa. */
  unoptimized?: boolean
  /** Eager-load and prioritize a largest-contentful-paint image. */
  priority?: boolean
}

/**
 * Render an accessible image backed by Ruvyxa's single WebP build output.
 */
export function Image({
  src,
  alt,
  width,
  height,
  unoptimized = false,
  priority = false,
  decoding = 'async',
  sizes,
  ...attributes
}: ImageProps): ReactElement {
  const outputSrc = unoptimized ? src : webpUrl(src)

  return (
    <img
      {...attributes}
      src={outputSrc}
      alt={alt}
      width={width}
      height={height}
      sizes={sizes}
      loading={priority ? 'eager' : 'lazy'}
      fetchPriority={priority ? 'high' : 'auto'}
      decoding={decoding}
    />
  )
}

/** Backward-compatible alias for `Image`. */
export const Picture = Image

function webpUrl(src: string): string {
  const marker = src.search(/[?#]/)
  const path = marker === -1 ? src : src.slice(0, marker)
  if (!/\.(?:png|jpe?g)$/i.test(path)) return src
  const converted = path.replace(/\.(?:png|jpe?g)$/i, '.webp')
  return marker === -1 ? converted : `${converted}${src.slice(marker)}`
}
