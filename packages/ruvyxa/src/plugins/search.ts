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
  /**
   * BCP 47 locale used for word segmentation and case folding, including
   * languages such as Thai.
   *
   * Set this. When it is absent the index is built for
   * {@link DEFAULT_INDEX_LOCALE} and the plugin reports `RUV2207`, because the
   * alternative -- letting ICU pick -- makes the emitted bytes a function of
   * the machine that ran the build. See {@link resolveIndexLocale}.
   */
  locale?: string
  stopWords?: string[]
  /** Ignore shorter terms. @default 2 */
  minTermLength?: number
}

/**
 * Locale the index is built for when a project does not name one.
 *
 * A search index is a build artifact, so it has to come out of the same source
 * identically on every machine -- the same property `localeCompare` and
 * host-locale case folding are banned outright for. Both of the ingredients
 * here are locale-sensitive: `Intl.Segmenter` decides where words begin, and
 * case folding decides which term a document is filed under. Passing
 * `undefined` to either does *not* mean "locale-independent"; it means "ask
 * ICU for this host's default", which is `th-TH` on the machine this framework
 * is developed on, `en-US` on GitHub's runners, and `tr-TR` on a Turkish
 * contributor's laptop -- where `Istanbul` folds to `ıstanbul` and lands under
 * a different key than it does anywhere else.
 *
 * So the fallback is a constant rather than the host's answer. `en` segments
 * on whitespace and punctuation, which is wrong for Thai and Japanese but
 * wrong *the same way everywhere*, and `RUV2207` says so out loud instead of
 * letting a project discover it when CI and a laptop disagree.
 */
export const DEFAULT_INDEX_LOCALE = 'en'

/**
 * Resolve a configured locale to a concrete one, or throw if it is malformed.
 *
 * Every locale-sensitive call in this file goes through the value this
 * returns, and both `segmentWords` and `createSearchIndexBody` take a required
 * `string`, so `undefined` cannot reach `Intl` by being forgotten at a call
 * site -- which is how it reached three of them before.
 */
export function resolveIndexLocale(locale: string | undefined, plugin: string): string {
  if (locale === undefined) return DEFAULT_INDEX_LOCALE
  try {
    Intl.Segmenter.supportedLocalesOf(locale)
  } catch {
    throw new TypeError(`${plugin}: locale must be a valid BCP 47 locale`)
  }
  return locale
}

/** The `RUV2207` a plugin reports when it had to fall back, or `undefined`. */
export function unsetLocaleDiagnostic(
  locale: string | undefined,
  option: string,
): { level: 'warning'; code: string; message: string } | undefined {
  if (locale !== undefined) return undefined
  return {
    level: 'warning',
    code: 'RUV2207',
    message:
      `${option} is not set, so the search index is built for ` +
      `"${DEFAULT_INDEX_LOCALE}". Word segmentation and case folding both ` +
      `depend on it, and a build must not read the host's locale: two ` +
      `machines would emit different bytes from the same source. Set it to ` +
      `the language the content is written in.`,
  }
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
  const locale = resolveIndexLocale(options.locale, 'searchIndex')
  const diagnostic = unsetLocaleDiagnostic(options.locale, 'searchIndex: locale')
  const stopWords = new Set(
    // `locale` is resolved above and is always a concrete string, so this folds
    // the same way on every machine. Passing `options.locale` straight through
    // would reach ICU as `undefined` and fold by the build host's locale.
    // oxlint-disable-next-line eslint/no-restricted-properties
    (options.stopWords ?? []).map((word) => word.toLocaleLowerCase(locale)),
  )

  const indexBody = (documents: SearchDocument[]) =>
    createSearchIndexBody(documents, locale, stopWords, minTermLength)

  return definePlugin({
    name: 'ruvyxa:search-index',
    ...(diagnostic ? { diagnostics: diagnostic } : {}),
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
  locale: string,
  stopWords: ReadonlySet<string>,
  minTermLength: number,
): string {
  const documents = normalizeSearchDocuments(input)
  const postings = new Map<string, Set<string>>()
  for (const document of documents) {
    const content = [document.title, document.text, ...(document.tags ?? [])].join(' ')
    for (const term of segmentWords(content, locale)) {
      // `locale` is a required parameter rather than `string | undefined` on
      // purpose: this is the fold that decides which key a document is filed
      // under, and `undefined` here would ask the build host's ICU instead.
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

/**
 * Split text into word-like segments for the given locale.
 *
 * `locale` is required. `new Intl.Segmenter(undefined)` resolves to the host's
 * default locale, which made the emitted index a function of the machine that
 * built it -- the defect this signature exists to prevent from coming back.
 */
export function segmentWords(value: string, locale: string): string[] {
  const Segmenter = Intl.Segmenter
  if (Segmenter) {
    return [...new Segmenter(locale, { granularity: 'word' }).segment(value)]
      .filter((part) => part.isWordLike)
      .map((part) => part.segment)
  }
  return value.match(/[\p{L}\p{N}]+/gu) ?? []
}
