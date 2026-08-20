import { definePlugin } from '@ruvyxa/core/plugin'
import type { RuvyxaPlugin } from '@ruvyxa/core/plugin'

import {
  compareStable,
  normalizePublicFilePath,
  validateAbsoluteHttpUrl,
  writePublicAsset,
} from './shared.js'

// ─── wellKnown ───────────────────────────────────────────────────────────────

export interface SecurityTxtOptions {
  /** How to reach the security team: `mailto:`, `https:`, or `tel:` URIs. Required. */
  contact: string | string[]
  /** When the record stops being authoritative. Required by RFC 9116. */
  expires: string | Date
  /** URL of the disclosure policy. */
  policy?: string
  /** URL of a page acknowledging reporters. */
  acknowledgments?: string
  /** URL of the team's PGP key. */
  encryption?: string
  /** URL of a hiring page for security roles. */
  hiring?: string
  /** BCP 47 language tags the team reads. */
  preferredLanguages?: string[]
  /** Canonical URL this file is served from. */
  canonical?: string
}

export interface WellKnownEntry {
  /** Path under `/.well-known/`, e.g. `apple-app-site-association`. */
  name: string
  /** File contents. An object is serialized as JSON. */
  body: string | Record<string, unknown>
  /** Overrides the type inferred from `body`. */
  contentType?: string
}

export interface WellKnownOptions {
  /** Generates `/.well-known/security.txt` per RFC 9116. */
  securityTxt?: SecurityTxtOptions
  /** Additional files published under `/.well-known/`. */
  entries?: WellKnownEntry[]
}

const CONTACT_SCHEMES = ['mailto:', 'https://', 'tel:']

/**
 * Publishes files under `/.well-known/`, served in development and written
 * into the production build.
 *
 * These are location-fixed by their specifications, so they cannot be produced
 * by an ordinary route — `.well-known` is a reserved prefix, and a scanner
 * looking for `security.txt` will not follow a redirect to somewhere prettier.
 */
export function wellKnown(options: WellKnownOptions = {}): RuvyxaPlugin {
  const files = new Map<string, { body: string; contentType: string }>()
  if (options.securityTxt) {
    files.set('/.well-known/security.txt', {
      body: createSecurityTxt(options.securityTxt),
      contentType: 'text/plain; charset=utf-8',
    })
  }
  for (const [index, entry] of (options.entries ?? []).entries()) {
    const at = `wellKnown.entries[${index}]`
    if (!entry || typeof entry !== 'object') throw new TypeError(`${at} must be an object`)
    if (typeof entry.name !== 'string' || entry.name === '' || entry.name.startsWith('/')) {
      throw new TypeError(`${at}.name must be a file name relative to /.well-known/`)
    }
    const outputPath = normalizePublicFilePath(`/.well-known/${entry.name}`, 'wellKnown')
    if (files.has(outputPath)) throw new TypeError(`wellKnown: duplicate entry ${outputPath}`)
    const isText = typeof entry.body === 'string'
    if (!isText && (!entry.body || typeof entry.body !== 'object')) {
      throw new TypeError(`${at}.body must be a string or JSON-serializable object`)
    }
    files.set(outputPath, {
      body: isText ? (entry.body as string) : `${JSON.stringify(entry.body, null, 2)}\n`,
      contentType:
        entry.contentType ??
        (isText ? 'text/plain; charset=utf-8' : 'application/json; charset=utf-8'),
    })
  }
  if (files.size === 0) {
    throw new TypeError('wellKnown: pass securityTxt and/or at least one entry')
  }
  // Sorted so the middleware route list a build writes out is byte-identical
  // on every machine, the same reason sitemap and feed entries are sorted.
  const paths = [...files.keys()].sort(compareStable)

  return definePlugin({
    name: 'ruvyxa:well-known',
    register({ http, build }) {
      http.onRequest({
        match: paths,
        handler({ request }) {
          const file = files.get(new URL(request.url).pathname)
          if (!file) return undefined
          return new Response(file.body, { headers: { 'content-type': file.contentType } })
        },
      })
      build.onComplete((context) => {
        for (const outputPath of paths) {
          writePublicAsset(context, outputPath, files.get(outputPath)?.body ?? '')
        }
      })
    },
  })
}

/**
 * Build the `security.txt` record.
 *
 * `Contact` and `Expires` are the two fields RFC 9116 makes mandatory, and an
 * expired record is worse than none — a reporter reads it as the team having
 * moved on. Both are required here rather than defaulted for that reason: a
 * generated expiry would silently lapse.
 */
function createSecurityTxt(options: SecurityTxtOptions): string {
  const contacts = Array.isArray(options.contact) ? options.contact : [options.contact]
  if (contacts.length === 0) throw new TypeError('wellKnown: securityTxt.contact is required')
  for (const [index, contact] of contacts.entries()) {
    if (
      typeof contact !== 'string' ||
      !CONTACT_SCHEMES.some((scheme) => contact.startsWith(scheme))
    ) {
      throw new TypeError(
        `wellKnown: securityTxt.contact[${index}] must be a mailto:, https://, or tel: URI`,
      )
    }
  }
  // Validated here rather than through `normalizeDate`, which bakes
  // `.publishedAt` into its message and answers in RFC 1123 form; RFC 9116
  // specifies `Expires` as an ISO 8601 timestamp.
  const expires =
    options.expires instanceof Date ? options.expires : new Date(options.expires as string)
  if (Number.isNaN(expires.getTime())) {
    throw new TypeError('wellKnown: securityTxt.expires must be a valid date')
  }

  const lines = contacts.map((contact) => `Contact: ${contact}`)
  lines.push(`Expires: ${expires.toISOString()}`)
  for (const [field, value] of [
    ['Encryption', options.encryption],
    ['Acknowledgments', options.acknowledgments],
    ['Policy', options.policy],
    ['Hiring', options.hiring],
    ['Canonical', options.canonical],
  ] as const) {
    if (value === undefined) continue
    validateAbsoluteHttpUrl(value, `wellKnown.securityTxt.${field.toLowerCase()}`)
    lines.push(`${field}: ${value}`)
  }
  const languages = options.preferredLanguages ?? []
  if (languages.length > 0) {
    if (
      languages.some(
        (tag) => typeof tag !== 'string' || !/^[A-Za-z]{2,3}(-[A-Za-z0-9]+)*$/.test(tag),
      )
    ) {
      throw new TypeError('wellKnown: securityTxt.preferredLanguages must be BCP 47 tags')
    }
    lines.push(`Preferred-Languages: ${languages.join(', ')}`)
  }
  return `${lines.join('\n')}\n`
}
