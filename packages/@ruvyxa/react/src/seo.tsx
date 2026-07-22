import type { ReactElement } from 'react'

export interface SeoAuthor {
  name: string
  url?: string
  /** @default "Person" */
  type?: 'Person' | 'Organization'
}

export interface SeoArticle {
  /** @default "Article" */
  type?: 'Article' | 'BlogPosting' | 'NewsArticle'
  publishedAt?: string
  updatedAt?: string
  authors?: readonly SeoAuthor[]
  section?: string
  tags?: readonly string[]
}

export interface SeoBreadcrumb {
  name: string
  url: string
}

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
  /** Explicit article facts used to generate Article JSON-LD. */
  article?: SeoArticle
  /** Ordered path from the site root to the current page. */
  breadcrumbs?: readonly SeoBreadcrumb[]
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
  article,
  breadcrumbs,
  jsonLd,
}: SeoProps): ReactElement {
  const structuredData = createStructuredData({
    title,
    description,
    canonical,
    image,
    article,
    breadcrumbs,
    jsonLd,
  })

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
      {structuredData ? (
        <script
          type="application/ld+json"
          dangerouslySetInnerHTML={{ __html: safeStructuredData(structuredData) }}
        />
      ) : null}
    </>
  )
}

function createStructuredData({
  title,
  description,
  canonical,
  image,
  article,
  breadcrumbs,
  jsonLd,
}: Pick<
  SeoProps,
  'title' | 'description' | 'canonical' | 'image' | 'article' | 'breadcrumbs' | 'jsonLd'
>): SeoProps['jsonLd'] {
  const generated: Array<Record<string, unknown>> = []

  if (article) {
    generated.push({
      '@context': 'https://schema.org',
      '@type': article.type ?? 'Article',
      headline: title,
      ...(description ? { description } : {}),
      ...(canonical ? { mainEntityOfPage: { '@type': 'WebPage', '@id': canonical } } : {}),
      ...(image ? { image: [image] } : {}),
      ...(article.publishedAt ? { datePublished: article.publishedAt } : {}),
      ...(article.updatedAt ? { dateModified: article.updatedAt } : {}),
      ...(article.authors?.length
        ? {
            author: article.authors.map((author) => ({
              '@type': author.type ?? 'Person',
              name: author.name,
              ...(author.url ? { url: author.url } : {}),
            })),
          }
        : {}),
      ...(article.section ? { articleSection: article.section } : {}),
      ...(article.tags?.length ? { keywords: [...article.tags] } : {}),
    })
  }

  if (breadcrumbs?.length) {
    generated.push({
      '@context': 'https://schema.org',
      '@type': 'BreadcrumbList',
      itemListElement: breadcrumbs.map((breadcrumb, index) => ({
        '@type': 'ListItem',
        position: index + 1,
        name: breadcrumb.name,
        item: breadcrumb.url,
      })),
    })
  }

  if (generated.length === 0) return jsonLd
  if (!jsonLd) return generated.length === 1 ? generated[0] : generated
  const custom = Array.isArray(jsonLd) ? jsonLd : [jsonLd]
  return [...generated, ...custom]
}

function safeStructuredData(value: NonNullable<SeoProps['jsonLd']>): string {
  return JSON.stringify(value).replace(/</g, '\\u003c')
}
