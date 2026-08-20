import { definePlugin } from '@ruvyxa/core/plugin'
import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

import { compareStable, normalizePublicFilePath, writePublicAsset } from './shared.js'

// ─── searchIndex ─────────────────────────────────────────────────────────────

export interface SearchDocument {
  id: string
  title: string
  url: string
  text: string
  tags?: string[]
}

export interface SearchIndexOptions {
  /** Static documents or a build-time loader. */
  documents: SearchDocument[] | (() => SearchDocument[] | Promise<SearchDocument[]>)
  /** @default "/search-index.json" */
  path?: string
  /** BCP 47 locale used for word segmentation, including languages such as Thai. */
  locale?: string
  stopWords?: string[]
  /** Ignore shorter terms. @default 2 */
  minTermLength?: number
}

/** Generates a compact static inverted index with locale-aware tokenization. */
export function searchIndex(options: SearchIndexOptions): RuvyxaPlugin {
  if (!options || (!Array.isArray(options.documents) && typeof options.documents !== 'function')) {
    throw new TypeError('searchIndex: documents must be an array or build-time loader')
  }
  const outputPath = normalizePublicFilePath(options.path ?? '/search-index.json', 'searchIndex')
  const minTermLength = options.minTermLength ?? 2
  if (!Number.isInteger(minTermLength) || minTermLength < 1 || minTermLength > 64) {
    throw new TypeError('searchIndex: minTermLength must be an integer from 1 to 64')
  }
  const stopWords = new Set(
    // The locale here is the project's own config value, not the host's ICU
    // default, so this folds identically on every machine that builds it.
    // Search terms should fold the way the project's language does.
    // oxlint-disable-next-line eslint/no-restricted-properties
    (options.stopWords ?? []).map((word) => word.toLocaleLowerCase(options.locale)),
  )

  const indexBody = (documents: SearchDocument[]) =>
    createSearchIndexBody(documents, options.locale, stopWords, minTermLength)

  return definePlugin({
    name: 'ruvyxa:search-index',
    register({ environment, http, build }) {
      // Same rule as `feed`: a static list answers from either environment, a
      // loader runs per request only in development, where there is no built
      // index to serve and a stale one would be worse than the cost.
      const staticDocuments = Array.isArray(options.documents) ? [...options.documents] : undefined
      if (staticDocuments || environment === 'development') {
        let cached: string | undefined
        http.onRequest({
          match: [outputPath],
          async handler({ request }) {
            if (new URL(request.url).pathname !== outputPath) return undefined
            const body = staticDocuments
              ? (cached ??= indexBody(staticDocuments))
              : indexBody(await resolveSearchDocuments(options))
            return new Response(body, {
              headers: { 'content-type': 'application/json; charset=utf-8' },
            })
          },
        })
      }
      build.onComplete(async (context) => {
        writePublicAsset(context, outputPath, indexBody(await resolveSearchDocuments(options)))
      })
    },
  })
}

async function resolveSearchDocuments(options: SearchIndexOptions): Promise<SearchDocument[]> {
  const documents =
    typeof options.documents === 'function' ? await options.documents() : [...options.documents]
  if (!Array.isArray(documents)) {
    throw new TypeError('searchIndex: document loader must return an array')
  }
  return documents
}

export function createSearchIndexBody(
  input: SearchDocument[],
  locale: string | undefined,
  stopWords: ReadonlySet<string>,
  minTermLength: number,
): string {
  const documents = normalizeSearchDocuments(input)
  const postings = new Map<string, Set<string>>()
  for (const document of documents) {
    const content = [document.title, document.text, ...(document.tags ?? [])].join(' ')
    for (const term of segmentWords(content, locale)) {
      // The locale here is the project's own config value, not the host's ICU
      // default, so this folds identically on every machine that builds it.
      // Search terms should fold the way the project's language does.
      // oxlint-disable-next-line eslint/no-restricted-properties
      const normalized = term.toLocaleLowerCase(locale)
      if (normalized.length < minTermLength || stopWords.has(normalized)) continue
      const ids = postings.get(normalized) ?? new Set<string>()
      ids.add(document.id)
      postings.set(normalized, ids)
    }
  }
  const terms = Object.fromEntries(
    [...postings.entries()]
      .sort(([left], [right]) => compareStable(left, right))
      .map(([term, ids]) => [term, [...ids].sort(compareStable)]),
  )
  return `${JSON.stringify({ version: 1, documents, terms })}\n`
}

function normalizeSearchDocuments(documents: SearchDocument[]): SearchDocument[] {
  const ids = new Set<string>()
  return documents
    .map((document, index) => {
      for (const field of ['id', 'title', 'url', 'text'] as const) {
        if (typeof document?.[field] !== 'string' || document[field].trim() === '') {
          throw new TypeError(
            `searchIndex: documents[${index}].${field} must be a non-empty string`,
          )
        }
      }
      if (ids.has(document.id)) throw new TypeError(`searchIndex: duplicate id ${document.id}`)
      if (
        document.tags !== undefined &&
        (!Array.isArray(document.tags) || document.tags.some((tag) => typeof tag !== 'string'))
      ) {
        throw new TypeError(`searchIndex: documents[${index}].tags must be an array of strings`)
      }
      ids.add(document.id)
      return { ...document, tags: document.tags ? [...document.tags] : undefined }
    })
    .sort((left, right) => compareStable(left.id, right.id))
}

export function segmentWords(value: string, locale: string | undefined): string[] {
  const Segmenter = Intl.Segmenter
  if (Segmenter) {
    return [...new Segmenter(locale, { granularity: 'word' }).segment(value)]
      .filter((part) => part.isWordLike)
      .map((part) => part.segment)
  }
  return value.match(/[\p{L}\p{N}]+/gu) ?? []
}
