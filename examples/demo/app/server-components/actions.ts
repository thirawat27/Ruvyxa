'use server'

import releases from './releases.json'

/**
 * Server functions this route's client island can call.
 *
 * The directive puts the whole module on the server side of the boundary: the
 * browser gets a *reference* for each export, not the code. Calling one posts
 * its arguments to the server, runs it there, and resolves to what it returned.
 *
 * Nothing below is in any browser bundle — not this file, not `releases.json`
 * it reads, and not `process`, which a browser does not have.
 */

/**
 * Look up a release by position and report where the answer came from.
 *
 * Deliberately not a counter held in a module variable. `ruvyxa dev` runs
 * several worker processes and a deployed build runs several instances, so such
 * a number would count differently depending on which one answered — which is
 * true of any server and worth not teaching by example.
 */
export async function describeRelease(index: number): Promise<{
  version: string
  note: string
  channel: string
}> {
  if (!Number.isInteger(index) || index < 0) {
    throw new Error('describeRelease expects a non-negative integer')
  }
  const entry = releases.entries[index % releases.entries.length]
  // Read from a module the browser bundle does not contain. The argument came
  // from the browser; the answer could only have been produced here.
  return { version: entry.version, note: entry.note, channel: releases.channel }
}

/**
 * The same lookup, shaped for a `<form>` rather than for a click handler.
 *
 * `useActionState` calls an action with the previous state and the submitted
 * `FormData`, which is also the shape React posts when the form is submitted by
 * a browser that is running no JavaScript at all — the arguments come from the
 * form's own fields instead of from a closure. That is the whole point of the
 * signature: the same function answers both.
 *
 * The returned string is what the page shows. With JavaScript it arrives in a
 * payload and React patches the `<output>`; without it, the submission renders
 * a new document with the answer already in place.
 */
export async function lookupRelease(_previous: string | null, form: FormData): Promise<string> {
  const version = String(form.get('version') ?? '').trim()
  if (!version) return 'type a version to look one up'
  const entry = releases.entries.find((candidate) => candidate.version === version)
  return entry
    ? `${entry.version} — ${entry.note} (${entry.channel})`
    : `no release is called ${version}`
}
