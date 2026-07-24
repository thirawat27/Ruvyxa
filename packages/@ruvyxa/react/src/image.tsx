import type { CSSProperties, ImgHTMLAttributes, ReactElement, SourceHTMLAttributes } from 'react'

/**
 * Responsive breakpoints, in pixels, used to build an automatic `srcset`.
 *
 * MUST equal `DEFAULT_VARIANT_WIDTHS` in
 * `crates/ruvyxa_cli/src/image_optimizer.rs`: the build emits a WebP at each of
 * these widths and this component references them by URL. A mismatch would make
 * the browser request a variant the build never produced.
 * `tests/packages/react/image-variants.test.mjs` asserts the two lists agree.
 */
export const DEFAULT_DEVICE_WIDTHS = [640, 750, 828, 1080, 1200, 1920, 2048, 3840] as const

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
  const outputSrcSet = resolveSrcSet({ src, srcSet, sizes, width, unoptimized, loader })
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

/**
 * Resolve the `srcset` for an `<img>`.
 *
 * An explicit `srcSet` always wins — the author has taken control. Otherwise,
 * when `sizes` signals a responsive layout and the intrinsic `width` is known,
 * build a `srcset` from the build's responsive variants. It is capped at the
 * intrinsic width because that is the largest file the optimizer produced: a
 * variant is only emitted for a breakpoint *narrower* than the source, and the
 * full-size WebP covers the top of the set.
 *
 * A custom loader or `unoptimized` opts out of Ruvyxa's build output, so the
 * author's `srcSet` (rewritten for a loader, verbatim for unoptimized) stands.
 */
function resolveSrcSet({
  src,
  srcSet,
  sizes,
  width,
  unoptimized,
  loader,
}: {
  src: string
  srcSet?: string
  sizes?: string
  width?: number
  unoptimized?: boolean
  loader?: ImageLoader
}): string | undefined {
  if (srcSet) {
    if (loader || unoptimized) return srcSet
    return rewriteLocalSrcSet(srcSet)
  }

  if (loader || unoptimized || !sizes || !width) return srcSet

  const base = webpUrl(src)
  // `webpUrl` only rewrites a local PNG/JPEG to `.webp`; it returns everything
  // else (remote URLs, protocol-relative, SVG, already-WebP) unchanged. A URL
  // it did not rewrite has no build-generated variants, so leave it alone
  // rather than fabricate `-640w.webp` links that would 404.
  if (base === src) return srcSet

  const entries = DEFAULT_DEVICE_WIDTHS.filter((deviceWidth) => deviceWidth < width).map(
    (deviceWidth) => `${variantUrl(base, deviceWidth)} ${deviceWidth}w`,
  )
  entries.push(`${base} ${width}w`)
  return entries.join(', ')
}

/**
 * URL of a responsive variant: `/hero.webp` at width 640 → `/hero-640w.webp`.
 *
 * Mirrors `variant_path()` in `crates/ruvyxa_cli/src/image_optimizer.rs`.
 */
function variantUrl(webpSrc: string, width: number): string {
  const marker = webpSrc.search(/[?#]/)
  const path = marker === -1 ? webpSrc : webpSrc.slice(0, marker)
  const suffix = marker === -1 ? '' : webpSrc.slice(marker)
  const variant = path.replace(/\.webp$/i, `-${width}w.webp`)
  return `${variant}${suffix}`
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
