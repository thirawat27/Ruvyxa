'use client'

import { PLUGIN_MARKER } from './plugin-marker'

export default function PluginLabPage() {
  return (
    <main>
      <h1>Plugin lab</h1>
      <p>These examples exercise framework-native plugin hooks without an API route.</p>
      <ul>
        <li>Request and response middleware: active on this page</li>
        <li>Module resolution: configured for the `~demo-plugin` virtual module</li>
        <li>Client source transform: {PLUGIN_MARKER}</li>
        <li>Build lifecycle: reports the generated route manifest</li>
      </ul>
    </main>
  )
}
