import type { ReactElement } from 'react'

export interface SeoProps {
  title: string
  description?: string
  canonical?: string
  image?: string
  imageAlt?: string
  siteName?: string
  type?: 'website' | 'article' | 'profile'
  locale?: string
  noindex?: boolean
  twitterCard?: 'summary' | 'summary_large_image'
  jsonLd?: Record<string, unknown> | Array<Record<string, unknown>>
}

/**
 * Render React 19 document metadata for search engines and social previews.
 * React hoists these tags into `<head>` during server rendering.
 */
export function Seo({
  title,
  description,
  canonical,
  image,
  imageAlt,
  siteName,
  type = 'website',
  locale,
  noindex = false,
  twitterCard = 'summary_large_image',
  jsonLd,
}: SeoProps): ReactElement {
  return (
    <>
      <title>{title}</title>
      {description ? <meta name="description" content={description} /> : null}
      {canonical ? <link rel="canonical" href={canonical} /> : null}
      <meta name="robots" content={noindex ? 'noindex, nofollow' : 'index, follow'} />
      <meta property="og:title" content={title} />
      {description ? <meta property="og:description" content={description} /> : null}
      <meta property="og:type" content={type} />
      {canonical ? <meta property="og:url" content={canonical} /> : null}
      {siteName ? <meta property="og:site_name" content={siteName} /> : null}
      {locale ? <meta property="og:locale" content={locale} /> : null}
      {image ? <meta property="og:image" content={image} /> : null}
      {image && imageAlt ? <meta property="og:image:alt" content={imageAlt} /> : null}
      <meta name="twitter:card" content={twitterCard} />
      <meta name="twitter:title" content={title} />
      {description ? <meta name="twitter:description" content={description} /> : null}
      {image ? <meta name="twitter:image" content={image} /> : null}
      {image && imageAlt ? <meta name="twitter:image:alt" content={imageAlt} /> : null}
      {jsonLd ? (
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{ __html: safeStructuredData(jsonLd) }}
        />
      ) : null}
    </>
  )
}

function safeStructuredData(value: SeoProps['jsonLd']): string {
  return JSON.stringify(value).replace(/</g, '\\u003c')
}
