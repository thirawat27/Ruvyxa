'use client'

import { useActionState } from 'react'

import { lookupRelease } from './actions'

/**
 * A form that works before — and without — its own JavaScript.
 *
 * `action` is a server function rather than a URL. React writes the reference
 * into hidden fields while rendering the form, so a browser that has not run a
 * line of this bundle can still submit it: the post goes to the page's own URL,
 * the server recognises the fields, runs `lookupRelease`, and answers with a new
 * document that already contains the result. Turn JavaScript off and the form
 * below keeps working.
 *
 * Once the bundle has loaded, React intercepts the submit instead — the same
 * function is called over `fetch` and only the `<output>` changes. `useActionState`
 * is what makes both spellings produce the same value: the hook's state is the
 * action's return value either way.
 */
export default function ReleaseForm() {
  const [answer, submit, pending] = useActionState(lookupRelease, null)

  return (
    <form action={submit} data-release-form>
      <label htmlFor="version">Version</label>{' '}
      <input id="version" name="version" defaultValue="1.0.31" size={16} />{' '}
      <button type="submit" disabled={pending}>
        look it up
      </button>{' '}
      <output data-release-answer>{answer ?? 'nothing looked up yet'}</output>
    </form>
  )
}
