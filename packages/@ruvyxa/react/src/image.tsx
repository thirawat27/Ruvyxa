import type { ImgHTMLAttributes, ReactElement } from 'react'

export type OptimizedImageFormat = 'avif' | 'webp'

export interface ImageProps extends Omit<
  ImgHTMLAttributes<HTMLImageElement>,
  'src' | 'alt' | 'width' | 'height' | 'loading' | 'fetchPriority'
> {
  /** Public image URL. PNG and JPEG files receive optimized build sidecars. */
  src: string
  /** Required accessible alternative text. Use an empty string for decorative images. */
  alt: string
  /** Intrinsic width prevents cumulative layout shift. */
  width: number
  /** Intrinsic height prevents cumulative layout shift. */
  height: number
  /** Preferred browser formats, in source order. */
  formats?: OptimizedImageFormat[]
  /** Eager-load and prioritize a largest-contentful-paint image. */
  priority?: boolean
}

/**
 * Render an accessible responsive picture backed by Ruvyxa build sidecars.
 * The original `src` remains the final fallback on every host.
 */
export function Image({
  src,
  alt,
  width,
  height,
  formats = ['avif', 'webp'],
  priority = false,
  decoding = 'async',
  sizes,
  ...attributes
}: ImageProps): ReactElement {
  const optimizable = /\.(?:png|jpe?g)(?:[?#].*)?$/i.test(src)
  const sources = optimizable ? [...new Set(formats)] : []

  return (
    <picture>
      {sources.map((format) => (
        <source
          key={format}
          srcSet={sidecarUrl(src, format)}
          type={`image/${format}`}
          sizes={sizes}
        />
      ))}
      <img
        {...attributes}
        src={src}
        alt={alt}
        width={width}
        height={height}
        sizes={sizes}
        loading={priority ? 'eager' : 'lazy'}
        fetchPriority={priority ? 'high' : 'auto'}
        decoding={decoding}
      />
    </picture>
  )
}

/** Alias that emphasizes art-direction/multi-format markup. */
export const Picture = Image

function sidecarUrl(src: string, format: OptimizedImageFormat): string {
  const marker = src.search(/[?#]/)
  return marker === -1
    ? `${src}.${format}`
    : `${src.slice(0, marker)}.${format}${src.slice(marker)}`
}
