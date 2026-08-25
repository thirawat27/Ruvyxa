import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  documentAssetsPrelude,
  documentStreamPrelude,
  routeMetaPrelude,
} from '../../../packages/ruvyxa/runtime/entry-templates.mjs'

/**
 * The document writers as a deployed function actually gets them.
 *
 * Evaluated from the emitted text rather than reimplemented, because the text
 * *is* the artifact: these functions exist only inside a compiled route
 * registry, and a test that rebuilt them here would prove that two copies agree
 * rather than that the shipped one is right. `ReactDomServer` is bound because
 * the stream prelude closes over it; nothing under test calls it.
 */
const emitted = new Function(
  'ReactDomServer',
  `${routeMetaPrelude()}\n${documentAssetsPrelude('<link rel="stylesheet" href="/app.css">')}\n${documentStreamPrelude()}\nreturn { __ruvyxaDocumentStream, __ruvyxaFinishDocument, __ruvyxaInjectDocumentAssets }`,
)({})

const HEAD = '<link rel="modulepreload" href="/a.js">'
const TAIL = '<script type="module" src="/a.js"></script>'

/** Feed one document through the transform in the chunks a caller chooses. */
async function streamed(chunks, { head = HEAD, tail = TAIL, lang = 'th' } = {}) {
  const encoder = new TextEncoder()
  const source = new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk))
      controller.close()
    },
  })
  const out = emitted.__ruvyxaDocumentStream(source, head, Promise.resolve(tail), lang)
  const decoder = new TextDecoder()
  let text = ''
  for await (const chunk of out) text += decoder.decode(chunk, { stream: true })
  return text + decoder.decode()
}

const DOCUMENT =
  '<html><head><title>t</title></head><body><main>hello</main><p>more</p></body></html>'

describe('a streamed document is the buffered document', () => {
  // The property the whole change rests on. A deployed build used to buffer the
  // render to a string and assemble it in one pass; it now assembles the two
  // ends around a stream. If the bytes differ at all, something the browser
  // parses moved — and every existing check would still pass, because they all
  // read a finished document.
  const splits = [
    [DOCUMENT],
    ['<html><head><title>t</title></head><body>', '<main>hello</main><p>more</p></body></html>'],
    [
      '<html><head><title>t</tit',
      'le></head><body><main>hello</main>',
      '<p>more</p></body></html>',
    ],
    ...[...DOCUMENT].map((_, index) => [DOCUMENT.slice(0, index + 1), DOCUMENT.slice(index + 1)]),
  ]

  it('whatever the chunk boundaries are', async () => {
    const expected = emitted.__ruvyxaFinishDocument(DOCUMENT, HEAD, TAIL, 'th')
    for (const chunks of splits) {
      assert.equal(await streamed(chunks), expected, JSON.stringify(chunks))
    }
  })

  it('including a render that produced a fragment rather than a page', async () => {
    // No `<head>` and no `<body>` anywhere: only the whole-document placement
    // can synthesise a page around it, so the transform has to fall back to it
    // rather than emit a naked fragment.
    const fragment = '<main>only this</main>'
    assert.equal(
      await streamed([fragment]),
      emitted.__ruvyxaFinishDocument(fragment, HEAD, TAIL, 'th'),
    )
    assert.equal(
      await streamed(['<main>only ', 'this</main>']),
      emitted.__ruvyxaFinishDocument(fragment, HEAD, TAIL, 'th'),
    )
  })

  it('including a document with a head but no closing body', async () => {
    const open = '<html><head></head><body><main>x</main>'
    assert.equal(await streamed([open]), emitted.__ruvyxaFinishDocument(open, HEAD, TAIL, 'th'))
  })

  it('and survives a multi-byte character split across a chunk boundary', async () => {
    // `TextDecoder` without `{ stream: true }` turns the halves of a Thai
    // character into two replacement characters, which the buffered path never
    // sees because it decodes once.
    const thai = '<html><head></head><body><main>สวัสดี</main></body></html>'
    const bytes = new TextEncoder().encode(thai)
    const cut = thai.indexOf('สวัสดี') + 1
    const decoder = new TextDecoder()
    const halves = [decoder.decode(bytes.slice(0, cut), { stream: true })]
    // Split the *bytes*, then hand the transform the raw halves.
    const source = new ReadableStream({
      start(controller) {
        controller.enqueue(bytes.slice(0, cut))
        controller.enqueue(bytes.slice(cut))
        controller.close()
      },
    })
    const out = emitted.__ruvyxaDocumentStream(source, HEAD, Promise.resolve(TAIL), 'th')
    const reader = new TextDecoder()
    let text = ''
    for await (const chunk of out) text += reader.decode(chunk, { stream: true })
    text += reader.decode()
    assert.equal(text, emitted.__ruvyxaFinishDocument(thai, HEAD, TAIL, 'th'))
    assert.equal(halves.length, 1)
  })

  it('waits for a tail that is not ready when the stream ends', async () => {
    // A server-components payload is complete only when the Flight render is,
    // which is after the caller has been sending bytes for a while.
    const late = new Promise((resolve) => setTimeout(() => resolve(TAIL), 20))
    const encoder = new TextEncoder()
    const source = new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode(DOCUMENT))
        controller.close()
      },
    })
    const out = emitted.__ruvyxaDocumentStream(source, HEAD, late, 'th')
    const decoder = new TextDecoder()
    let text = ''
    for await (const chunk of out) text += decoder.decode(chunk, { stream: true })
    assert.equal(
      text + decoder.decode(),
      emitted.__ruvyxaFinishDocument(DOCUMENT, HEAD, TAIL, 'th'),
    )
  })
})

describe('the shell leaves before the rest is written', () => {
  it('emits the opened document without waiting for the closing chunk', async () => {
    const encoder = new TextEncoder()
    let release = null
    const source = new ReadableStream({
      start(controller) {
        controller.enqueue(encoder.encode('<html><head></head><body><main>shell</main>'))
        controller.enqueue(encoder.encode('<p>filled in</p>'))
        release = () => {
          controller.enqueue(encoder.encode('</body></html>'))
          controller.close()
        }
      },
    })
    const out = emitted.__ruvyxaDocumentStream(source, HEAD, Promise.resolve(TAIL), null)
    const reader = out.getReader()
    const decoder = new TextDecoder()

    // The transform holds back only what could still precede a closing
    // `</body>`, so the opened shell goes out on the first read rather than
    // waiting for the boundary that is still resolving.
    const first = decoder.decode((await reader.read()).value)
    assert.match(first, /^<!doctype html><html><head>/i)
    assert.match(first, /shell/)
    assert.doesNotMatch(first, /filled in/)

    release()
    let rest = ''
    for (;;) {
      const next = await reader.read()
      if (next.done) break
      rest += decoder.decode(next.value, { stream: true })
    }
    assert.match(rest, /filled in/)
    assert.ok(rest.lastIndexOf(TAIL) < rest.lastIndexOf('</body>'))
  })
})
