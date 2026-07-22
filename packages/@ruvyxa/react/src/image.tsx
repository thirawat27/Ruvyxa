import type { CSSProperties, ImgHTMLAttributes, ReactElement, SourceHTMLAttributes } from 'react'

/** Parameters supplied to a custom image CDN loader. */
export interface ImageLoaderProps {
  src: string
  width?: number
  quality?: number
}

/** Returns the URL served by an image CDN or other external image service. */
export type ImageLoader = (props: ImageLoaderProps) => string

interface ImageBaseProps extends Omit<
  ImgHTMLAttributes<HTMLImageElement>,
  'alt' | 'fetchPriority' | 'height' | 'loading' | 'src' | 'width'
> {
  /** Public image URL. Local PNG/JPEG URLs are rewritten to their build-time WebP output. */
  src: string
  /** Required accessible alternative text. Use an empty string for decorative images. */
  alt: string
  /** Keep local source URLs unchanged. Remote URLs are unchanged by default. */
  unoptimized?: boolean
  /** Eager-load and prioritize a largest-contentful-paint image. */
  priority?: boolean
  /** Fill the positioned parent while retaining browser-native image behavior. */
  fill?: boolean
  /** Generate the URL through an image CDN without enabling Ruvyxa runtime transforms. */
  loader?: ImageLoader
  /** Passed to a custom loader; local build output always uses configured build quality. */
  quality?: number
  /** Overrides the default lazy loading when the image is not priority. */
  loading?: 'eager' | 'lazy'
  /** Overrides the default fetch priority when the image is not priority. */
  fetchPriority?: 'auto' | 'high' | 'low'
}

export type ImageProps =
  | (ImageBaseProps & {
      fill?: false | undefined
      /** Intrinsic width prevents cumulative layout shift. */
      width: number
      /** Intrinsic height prevents cumulative layout shift. */
      height: number
    })
  | (ImageBaseProps & {
      fill: true
      width?: number
      height?: number
    })

/** A browser-native source used for responsive art direction. */
export interface PictureSource extends Omit<SourceHTMLAttributes<HTMLSourceElement>, 'srcSet'> {
  srcSet: string
  /** Do not rewrite local PNG/JPEG URLs in this source set. */
  unoptimized?: boolean
}

export type PictureProps = ImageProps & {
  /** Sources evaluated by the browser before the fallback image. */
  sources: readonly PictureSource[]
}

/**
 * Render an accessible image backed by Ruvyxa's static WebP build output.
 *
 * `fill` requires the parent to establish a CSS positioning context, matching
 * native absolute-positioned image behavior without a framework wrapper.
 */
export function Image({
  src,
  alt,
  width,
  height,
  unoptimized = false,
  priority = false,
  fill = false,
  loader,
  quality,
  decoding = 'async',
  loading,
  fetchPriority,
  sizes,
  srcSet,
  style,
  ...attributes
}: ImageProps): ReactElement {
  const outputSrc = resolveImageUrl({ src, width, quality, unoptimized, loader })
  const outputSrcSet = loader || unoptimized ? srcSet : rewriteLocalSrcSet(srcSet)
  const outputStyle = fill ? fillStyle(style) : style

  return (
    <img
      {...attributes}
      src={outputSrc}
      srcSet={outputSrcSet}
      alt={alt}
      width={width}
      height={height}
      sizes={sizes}
      loading={priority ? 'eager' : (loading ?? 'lazy')}
      fetchPriority={priority ? 'high' : (fetchPriority ?? 'auto')}
      decoding={decoding}
      style={outputStyle}
    />
  )
}

/**
 * Render native `<picture>` sources for art direction with the same static
 * WebP URL rewriting as `Image`. No responsive variants are generated.
 */
export function Picture({ sources, ...image }: PictureProps): ReactElement {
  return (
    <picture>
      {sources.map(({ srcSet, unoptimized = false, type, ...source }) => {
        const rewrittenSrcSet = unoptimized ? srcSet : rewriteLocalSrcSet(srcSet)
        const changed = rewrittenSrcSet !== srcSet
        return (
          <source
            {...source}
            key={`${source.media ?? ''}:${srcSet}`}
            srcSet={rewrittenSrcSet}
            type={changed ? 'image/webp' : type}
          />
        )
      })}
      <Image {...image} />
    </picture>
  )
}

function resolveImageUrl({
  src,
  width,
  quality,
  unoptimized,
  loader,
}: {
  src: string
  width?: number
  quality?: number
  unoptimized?: boolean
  loader?: ImageLoader
}): string {
  if (loader) return loader({ src, width, quality })
  return unoptimized ? src : webpUrl(src)
}

function fillStyle(style: CSSProperties | undefined): CSSProperties {
  return {
    position: 'absolute',
    inset: 0,
    width: '100%',
    height: '100%',
    ...style,
  }
}

function rewriteLocalSrcSet(srcSet: string | undefined): string | undefined {
  if (!srcSet) return srcSet
  return srcSet.replace(/(^|,\s*)(\/[^\s,]+)/g, (_, prefix: string, url: string) => {
    return `${prefix}${webpUrl(url)}`
  })
}

function webpUrl(src: string): string {
  if (!src.startsWith('/') || src.startsWith('//')) return src
  const marker = src.search(/[?#]/)
  const path = marker === -1 ? src : src.slice(0, marker)
  if (!/\.(?:png|jpe?g)$/i.test(path)) return src
  const converted = path.replace(/\.(?:png|jpe?g)$/i, '.webp')
  return marker === -1 ? converted : `${converted}${src.slice(marker)}`
}
