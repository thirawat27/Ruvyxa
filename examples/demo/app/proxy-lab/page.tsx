export default function ProxyLabPage() {
  return (
    <main>
      <h1>Proxy lab</h1>
      <p>These examples exercise the request pipeline declared in ruvyxa.config.ts.</p>
      <ul>
        <li>headers(): every response under /proxy-lab carries x-demo-headers-rule</li>
        <li>proxy.handler: forwards this page with an x-demo-proxy-request header</li>
        <li>proxy.handler: answers /proxy-lab/blocked with 403 before any route</li>
        <li>redirects() and rewrites(): see the config and the docs on configuration</li>
      </ul>
    </main>
  )
}
