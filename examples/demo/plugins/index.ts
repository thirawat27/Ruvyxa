import { bundleBudget, sitemap } from 'ruvyxa/plugins'

import buildPipeline from './build-pipeline'
import pageObservability from './page-observability'
import renderModeBadges from './render-mode-badges'

export const demoPlugins = [
  pageObservability,
  renderModeBadges,
  buildPipeline,
  sitemap({ siteUrl: 'https://demo.ruvyxa.dev', robots: true }),
  bundleBudget({ maxChunkKb: 1024, maxTotalKb: 4096 }),
]
