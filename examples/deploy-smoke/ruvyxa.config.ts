import { config, type RuvyxaConfig } from 'ruvyxa/config'

/**
 * Nothing here is set for its own sake. The fixture exists to be *built by an
 * adapter and then run*, so the configuration is the default one — anything
 * else would mean the smoke proves something about this file rather than about
 * the server the adapter emitted.
 */
const settings: RuvyxaConfig = {
  appDir: 'app',
  outDir: '.ruvyxa',
}

export default config(settings)
