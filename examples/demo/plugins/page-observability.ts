import { plugin } from 'ruvyxa/config'

/** Adds request/response metadata to the plugin showcase page. */
export default plugin('demo-page-observability', {
  routes: ['/plugin-lab'],

  onRequest(request) {
    const headers = new Headers(request.headers)
    headers.set('x-demo-plugin-request', 'active')
    return new Request(request, { headers })
  },

  onResponse(request, response) {
    const headers = new Headers(response.headers)
    headers.set('x-demo-plugin-response', 'active')
    headers.set('x-demo-plugin-route', new URL(request.url).pathname)
    return new Response(response.body, {
      status: response.status,
      statusText: response.statusText,
      headers,
    })
  },
})
