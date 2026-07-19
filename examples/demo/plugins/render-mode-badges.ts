import { plugin } from 'ruvyxa/config'

const renderModes: Record<string, string> = {
  '/static-page': 'static',
  '/ssg-blog': 'ssg',
  '/isr-page': 'isr',
  '/csr-page': 'csr',
  '/ppr-page': 'ppr',
}

/** Labels different rendering strategies through the normal response middleware. */
export default plugin('demo-render-mode-badges', {
  routes: ['/static-page', '/ssg-blog*', '/isr-page', '/csr-page', '/ppr-page'],

  onResponse(request, response) {
    const pathname = new URL(request.url).pathname
    const mode = Object.entries(renderModes).find(([prefix]) => pathname.startsWith(prefix))?.[1]
    if (!mode) return response

    const headers = new Headers(response.headers)
    headers.set('x-demo-render-mode', mode)
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    })
  },
})
