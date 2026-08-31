# Configuration และ environment

> **เป้าหมายของ tutorial:** เปลี่ยนแอป development เป็นแอปที่กำหนดค่าอย่างตั้งใจและเก็บ secret
> อย่างปลอดภัย **เริ่มจาก:** UI และ asset ใน
> [UI, navigation, metadata และ asset](06-ui-navigation-metadata-and-assets.md) **Checkpoint:**
> commit environment example ที่ปลอดภัย เก็บ secret ไว้เฉพาะ server และรัน app check

`ruvyxa.config.ts` ถูกประเมินโดย configuration renderer แล้ว validate ใช้ `config()` จาก
`ruvyxa/config` เพื่อเขียนแบบ typed ชื่อ configuration ด้านล่างมาจาก `RuvyxaConfig` และ nested
source type ของมัน

## Option หลัก

| Key                                                                                             | Type / ค่าเริ่มต้น                                     | ผลกระทบ                                                                  |
| ----------------------------------------------------------------------------------------------- | ------------------------------------------------------ | ------------------------------------------------------------------------ |
| `appDir`, `outDir`                                                                              | string                                                 | ตำแหน่ง app source และ generated output                                  |
| `runtime`                                                                                       | `node \| bun \| deno \| edge \| static`, ปริยาย `node` | นโยบาย runtime/target                                                    |
| `typedRoutes`                                                                                   | boolean, ปริยาย `false`                                | สร้าง `.ruvyxa/types/routes.d.ts` เพื่อตรวจ `<Link href>` กับ route จริง |
| `server.host`, `server.port`                                                                    | string, number                                         | address ที่ฟัง ดู [Listening address](#listening-address)                |
| `build.minify`, `map`, `treeShake`, `manifest`, `warm`, `prerenderCache`                        | boolean; cache ปริยาย true                             | พฤติกรรม compiler/build artifact                                         |
| `build.split`                                                                                   | `single \| route \| manual`                            | นโยบาย bundle splitting                                                  |
| `build.workers`                                                                                 | number                                                 | build parallelism ดูหมายเหตุด้านล่าง                                     |
| `render.strategy`, `render.revalidate`                                                          | strategy, seconds                                      | นโยบาย page rendering ปริยาย                                             |
| `cache.routes`, `cache.css`, `cache.dir`, `cache.handler`, `cache.maxEntries`, `cache.maxBytes` | boolean/string                                         | setting route/CSS/cache directory                                        |

## แผนที่ option แบบครบกลุ่ม

| กลุ่ม         | Key                                                                                                                              | การตัดสินใจเชิงปฏิบัติการ                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        |
| ------------- | -------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Root          | `appDir`, `outDir`, `runtime`, `typedRoutes`, `reactCompiler`                                                                    | ใช้ default จนกว่าจะต้องเปลี่ยน source/output layout หรือ target `runtime` คือ `node`, `bun`, `deno`, `edge` หรือ `static`; CLI target override ได้ `typedRoutes` ต้องมี `.ruvyxa/types/**/*.d.ts` ใน `include` ของ tsconfig ด้วย ส่วน `react` และ `typescript` ตัว validator รับค่าอยู่แต่ไม่มีอะไรอ่าน — ตั้ง strictness ใน `tsconfig.json` ของโปรเจกต์เอง ส่วน `reactCompiler` ปิดไว้เป็นค่าเริ่มต้น — ดู [React Compiler](#react-compiler)                                                                                                   |
| CSS และ debug | `css.entries`, `debug.overlay`, `debug.traces`                                                                                   | `entries` สำหรับ global style แบบ project-relative ที่ไม่มี module import debug flag เปลี่ยน development diagnostic ไม่ใช่ production access control                                                                                                                                                                                                                                                                                                                                                                                             |
| Build         | `minify`, `map`, `treeShake`, `split`, `workers`, `jsx`, `target`, `manifest`, `warm`, `prerenderCache`                          | `split` เป็น `single`, `route` หรือ `manual`; `jsx` เป็น `classic` หรือ `automatic`; `target` เป็น `es2015` ถึง `es2026` หรือ `esnext` (ค่าปริยาย) และ compiler ทั้งสองตัวใช้ค่านี้จริง target ที่ต่ำกว่า syntax ที่โค้ดใช้อาจต้องพึ่ง runtime helper — Ruvyxa ไม่ได้ ship helper runtime มาด้วย ดังนั้นโมดูลที่ต้องใช้ helper จะทำให้ build ล้มเหลวพร้อมบอกชื่อ helper แทนที่จะ emit import ที่ resolve ไม่ได้ โค้ดแอปพลิเคชันทั่วไปคอมไพล์ได้โดยไม่ต้องใช้ helper ที่ `es2022` ขึ้นไป ใช้ source map อย่างตั้งใจเพราะอาจเปิดเผย source content |
| Rendering     | `render.strategy`, `render.revalidate`                                                                                           | Strategy คือ `ssr`, `ssg`, `isr`, `csr` หรือ `ppr` strategy ปริยายคือ SSR และ revalidation ปริยาย 60 วินาที                                                                                                                                                                                                                                                                                                                                                                                                                                      |
| Image         | `optimize`, `quality`, `lossless`, `keepOriginal`, `variantWidths`, `workers`, `effort`, `onDemand.enabled`, `onDemand.maxWidth` | Default คือ optimize true, quality 82, lossless false, keep-original false, ไม่สร้าง prebuilt variant, worker 0 (จำนวน CPU ที่ใช้ได้) และ effort 4 จึงได้ WebP หนึ่งไฟล์ต่อต้นฉบับ ส่วน on-demand image แบบ object เปิดโดยปริยายและ max width 3840                                                                                                                                                                                                                                                                                               |
| i18n          | `locales`, `defaultLocale`, `localeParam`, `detectLocale`, `cookie`                                                              | `locales` และ `defaultLocale` จำเป็นเมื่อกำหนด i18n param ปริยาย `lang`, detection true, cookie `RUVYXA_LOCALE`                                                                                                                                                                                                                                                                                                                                                                                                                                  |
| Site          | `site.url`, `site.sitemap`, `site.robots`                                                                                        | Sitemap ตั้ง `exclude`, `additionalPaths`, `defaults` และ `entries` ที่เพิ่ม metadata ได้; robots ตั้ง rule, sitemap URL และ host ได้                                                                                                                                                                                                                                                                                                                                                                                                            |
| Middleware    | `builtin.cors`, `builtin.timing`, `builtin.log`, `builtin.rate`, `builtin.headers`, `workers`, `timeoutMs`                       | CORS มี origins/methods/headers/credentials/maxAge built-in rate ต้องมี `max`, `window`, `key` แบบเลือกได้ plugin worker 1–8; timeout ปริยาย 30,000 ms และสูงสุด 300,000                                                                                                                                                                                                                                                                                                                                                                         |
| Integration   | `adapter`, `adapterOptions`, `plugins`                                                                                           | `adapter` เก็บ adapter ที่สร้างไว้แล้ว ส่วน `adapterOptions` ใช้ตั้งค่า adapter ที่เลือกด้วยชื่อแทน (ดู [Configuring an adapter selected by name](#configuring-an-adapter-selected-by-name)) การตั้งทั้งสองพร้อมกันเป็น error ส่วน `plugins` เป็น array ของ `RuvyxaPlugin`                                                                                                                                                                                                                                                                       |

## Runtime selection

สำหรับ JavaScript process ให้ใช้ `node`, `bun` หรือ `deno` โดย `--runtime` มีลำดับสูงสุด ตามด้วย
`RUVYXA_RUNTIME` และ `runtime` ใน `ruvyxa.config.ts` หาก project ไม่ระบุ runtime การเรียกผ่าน
`bun run` หรือ `deno task` เป็นเพียง hint; หากไม่มีให้ตรวจ Node, Bun แล้ว Deno ตามลำดับ `edge` และ
`static` เป็น build target ไม่ใช่ JavaScript worker host

Deno รัน trusted local project configuration และ plugin พร้อม permission ที่ต้องใช้
(`deno run -A --no-prompt --node-modules-dir=manual`) อย่าเลือกใช้กับ project code ที่ไม่น่าเชื่อถือ

## Listening address

`--host` และ `--port` มีลำดับสูงสุด ตามด้วย environment variable `HOST` และ `PORT` แล้วจึงเป็น
`server.host` และ `server.port` ใน `ruvyxa.config.ts` และค่าปริยายของแต่ละคำสั่งเป็นลำดับสุดท้าย

ที่ environment ชนะ config file เป็นความตั้งใจ: platform แบบ managed จะฉีด `PORT`
เข้ามาและคาดหวังให้ process ใช้ค่านั้น ส่วน `ruvyxa.config.ts` ที่ commit ไว้ใน repository
ไม่มีทางรู้เลขนั้นได้ หาก `PORT` ไม่ใช่ตัวเลขระหว่าง 0 ถึง 65535
คำสั่งจะล้มเหลวแทนที่จะถอยไปใช้ค่าอื่น เพื่อให้ deployment ที่ตั้งค่าผิดรายงานสาเหตุออกมา
แทนที่จะเห็นเพียง health check ที่ไม่ผ่าน

| คำสั่ง                           | host ปริยาย | port ปริยาย |
| -------------------------------- | ----------- | ----------- |
| `ruvyxa dev`                     | `localhost` | `3000`      |
| `ruvyxa start`, `ruvyxa preview` | `0.0.0.0`   | `3000`      |

`start` และ `preview` bind ทุก interface เพราะ container จะ route ไปยัง address ของ container ไม่ใช่
loopback ของมัน — production server ที่ bind `localhost` จึงไม่ตอบอะไรเลยจากภายนอก ค่านี้ตรงกับ
standalone server ที่ `ruvyxa build` สร้างออกมา ซึ่งอ่าน `PORT` และ `HOST` แบบเดียวกันมาตลอด ใช้
`--host localhost` หากต้องการรัน production ในเครื่องโดยไม่เปิดออกสู่เครือข่าย

## Configuring an adapter selected by name

adapter เข้าถึง build ได้สองทาง และแต่ละทางรับ option ต่างกัน

สร้างไว้ใน config — option ส่งเข้า factory โดยตรง:

```ts
import { config } from 'ruvyxa/config'
import { render } from '@ruvyxa/adapter-render'

export default config({ adapter: render({ serviceName: 'checkout-api' }) })
```

เลือกด้วยชื่อ — `ruvyxa build --adapter render`, `RUVYXA_ADAPTER=render` หรือการตรวจ platform จาก
environment ของ hosting กรณีนี้ไม่มี factory call ให้ส่ง option เข้าไป `adapterOptions`
จึงทำหน้าที่เป็น argument ของ call นั้น:

```ts
import { config } from 'ruvyxa/config'

export default config({ adapterOptions: { serviceName: 'checkout-api' } })
```

รูปแบบที่สองคือสิ่งที่ทำให้ zero-config deploy ยังตั้งค่าได้: project ไม่ระบุ adapter, platform
เลือกให้ และ option ยังมีผลอยู่ adapter จะ validate option เอง ดังนั้นค่าที่ adapter ไม่รับจะทำให้
build ล้มเหลวพร้อม diagnostic ของ adapter ตัวนั้น

การตั้ง `adapter` และ `adapterOptions` พร้อมกันเป็น error ไม่ใช่กฎลำดับความสำคัญ เพราะ adapter
ที่สร้างแล้วถือ option ของตัวเองอยู่ และจะไม่มีอะไรอ่าน option ชุดที่สอง

## Production configuration example

เริ่มจาก configuration ที่แคบนี้ แล้วเพิ่มเฉพาะ feature ที่ application ทดสอบแล้ว value ทั้งหมดเป็น
option name ที่รองรับ ให้แทน origin ตัวอย่างก่อน release

```ts
import { config } from 'ruvyxa/config'
import { requireEnv, securityHeaders } from 'ruvyxa/plugins'

export default config({
  site: {
    url: 'https://app.example.com',
    title: 'Example',
    description: 'Product notes and guides',
    language: 'th',
    sitemap: true,
    robots: true,
  },
  content: true,
  build: { minify: true, map: false, treeShake: true, split: 'route', prerenderCache: true },
  security: { actionLimit: 1_048_576, apiLimit: 10_485_760, sameOrigin: true, fetchMeta: true },
  plugins: [
    requireEnv(['DATABASE_URL', 'RUVYXA_AUTH_SECRET']),
    securityHeaders({ contentSecurityPolicy: { 'default-src': ["'self'"] } }),
  ],
})
```

`requireEnv` validate name ตอนท้าย production build จึงต้องตั้ง required value ใน build environment
เดียวกัน มันไม่อ่าน secret เข้า browser code CSP มักต้องเพิ่ม source สำหรับ analytics, image, font
หรือ API; ทดสอบทุก route หลังจำกัด policy

```ts
import { config } from 'ruvyxa/config'

export default config({
  build: { minify: true, map: false, treeShake: true, split: 'route', prerenderCache: true },
  render: { strategy: 'ssr', revalidate: 60 },
  image: {
    optimize: true,
    quality: 82,
    variantWidths: [640, 1200],
    onDemand: { enabled: true, maxWidth: 1920 },
  },
  i18n: { locales: ['en', 'th'], defaultLocale: 'en' },
})
```

## React Compiler

`reactCompiler: true` จะรัน React Compiler ตัวจริงกับ component ของคุณก่อน Oxc transform ของ Ruvyxa
เอง ทำให้การ memoize ถูกอนุมานให้แทนที่จะเขียน `useMemo` กับ `useCallback` เอง

```ts
export default config({ reactCompiler: true })
```

ค่าเริ่มต้นคือ **ปิด** และตั้งใจไม่มี option ย่อยของตัวเอง มีสองข้อที่ควรรู้ก่อนเปิดใช้:

- มันรันใน **inference mode** ซึ่งเป็นค่าเริ่มต้นของ upstream และ target React 19 อันเป็นเวอร์ชัน
  peer ที่ Ruvyxa ต้องการอยู่แล้ว ที่นี่ไม่มีการ opt-in/opt-out รายไฟล์ — component ใช้ directive
  ของ compiler เองในการหลีกเลี่ยง
- ไฟล์ config ของ Babel จะถูก **ละเว้น** (`babelrc: false`, `configFile: false`)
  ซึ่งตั้งใจไว้เช่นนั้น: `.babelrc` ในโปรเจกต์อาจทำให้ lane ฝั่ง server กับ lane ฝั่ง client compile
  component เดียวกันไม่เหมือนกัน ซึ่งเป็นความต่างที่มักโผล่มาในรูป hydration mismatch บน production
  เท่านั้น

output ที่ compile แล้วถูก key ด้วยเนื้อหาเหมือน transform อื่นทุกตัว การตั้งค่านี้จึงยังใช้ build
cache ได้ตามปกติ เปิดใช้ แล้วรัน `ruvyxa build` เพื่อเทียบดู — มันเปลี่ยน JavaScript ที่ emit ออกมา
ไม่ใช่ semantics ของโค้ดที่ถูกต้องอยู่แล้ว

## Compiler สำหรับ Markdown และ MDX

`markdown` ตั้งค่า pipeline `@mdx-js/mdx` ชุดเดียวที่ development, SSR/SSG, adapter และ native
production client bundle ใช้ร่วมกัน `gfm` เปิดเป็นค่าเริ่มต้น ส่วน `remarkPlugins`, `rehypePlugins`
และ `recmaPlugins` รับ unified plugin หรือ tuple `[plugin, options]` และ `remarkRehypeOptions`
ใช้ส่งค่าภาษา footnote กับค่าของ bridge อ่านตัวอย่างเต็มและ contract ของ frontmatter/heading ได้ที่
[Routing และ rendering](04-routing-rendering.md)

## Security, middleware, site และ plugin

`security.actionLimit` ปริยาย 1,048,576 byte; `security.apiLimit` ปริยาย 10,485,760 byte;
`security.pluginLimit` ปริยาย 33,554,432 และจำกัดสูงสุด 268,435,456 `security.actionRateLimit`
ปริยาย 600 request ใน 60 วินาที `trustedProxyIps` รับ IPv4/IPv6 แบบ exact หรือ CIDR range; เฉพาะ
non-loopback proxy ที่ตั้งค่าเท่านั้นที่ส่ง forwarded client/protocol header ได้

`middleware` มี built-in (`cors`, `timing`, `log`, `rate`, `headers`) และ TypeScript plugin
`build.workers` ควรปล่อยไม่ตั้งค่า เมื่อไม่ตั้ง การ bundle route จะปรับตามเครื่อง:
ค่าที่น้อยกว่าระหว่างจำนวน core (เคารพ `RAYON_NUM_THREADS`) กับจำนวนที่ memory ว่างรองรับได้ การ pin
ตัวเลขไว้จะจำกัดเครื่องใหญ่ — ค่า 4 ใช้แค่ 4 worker บนเครื่อง 16 core — และ starter template
ไม่ส่งค่านี้มาแล้ว การตั้งค่าจะลด CPU budget เท่านั้น ส่วนขอบเขต memory ยังบังคับอยู่ ค่าที่ copy
มาจากโปรเจกต์อื่นจึงทำให้ CI container ที่จำกัด memory ขอเกินที่มีไม่ได้

`workers` (1–8) กับ `timeoutMs` (ปริยาย 30,000, สูงสุด 300,000) `site` ตั้งค่า `sitemap.xml` และ
`robots.txt` ตอน build; exact app route หรือไฟล์ชื่อเดียวกันใน `public/` จะระงับ core generator
`plugins` คือ array ของ `RuvyxaPlugin`

## สร้าง content artifact โดยไม่ต้องต่อ plugin เอง

route Markdown และ MDX ใช้งานได้โดยไม่ต้องตั้ง `content` ให้เปิด `content: true` เฉพาะเมื่อ site
ต้องการ `/content.json`, `/search-index.json`, `/rss.xml`, `/sitemap.xml` และ `/llms.txt` เพิ่มด้วย
content engine จะใช้ `site.url`, `site.title`, `site.description` และ `site.language` ร่วมกัน
จึงไม่ต้อง import plugin หรือกรอกข้อมูล site ซ้ำ

```ts
export default config({
  site: {
    url: 'https://example.com',
    title: 'Example Docs',
    description: 'คู่มือสำหรับ Example',
    language: 'th',
  },
  content: {
    engine: {
      exclude: ['/drafts/*'],
      minTermLength: 3,
      llmsPath: false,
    },
  },
})
```

plugin `contentEngine(options)` แบบเดิมยังรองรับสำหรับ advanced/programmatic composition แต่ห้าม
ตั้งทั้งสองรูปแบบใน application เดียวกัน

## Environment variable

| Variable                                                                                                                                                                 | วัตถุประสงค์ที่ยืนยันจากหลักฐาน                                                                                                                |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------------------------------- |
| `RUVYXA_SITE_URL`                                                                                                                                                        | fallback canonical origin ของ site discovery                                                                                                   |
| `RUVYXA_RUNTIME`                                                                                                                                                         | CLI/runtime override (`node`, `bun` หรือ `deno`) ที่ dev/build ใช้                                                                             |
| `RUVYXA_ADAPTER`                                                                                                                                                         | build adapter selection override                                                                                                               |
| `RUVYXA_BUILD_CACHE_DIR`                                                                                                                                                 | shared build cache directory override                                                                                                          |
| `RUVYXA_RENDER_CACHE_SIZE`                                                                                                                                               | render-cache capacity                                                                                                                          |
| `RUVYXA_WORKER_POOL_SIZE`, `RUVYXA_WORKER_TIMEOUT_MS`, `RUVYXA_WORKER_MAX_CONCURRENCY`, `RUVYXA_WORKER_MAX_QUEUE`, `RUVYXA_MEMORY_LIMIT_MB`, `RUVYXA_WORKER_SHUTDOWN_MS` | worker-pool operational control                                                                                                                |
| `RUVYXA_MAX_CONCURRENCY`, `RUVYXA_MAX_QUEUE`                                                                                                                             | จำนวน render ที่ `ruvyxa start` รันพร้อมกันและจำนวนที่รอได้ ตั้ง concurrency เป็น `0` เพื่อปิด admission ส่วน `ruvyxa dev` ปิดไว้เป็นค่าปริยาย |
| `RUVYXA_DRAIN_DELAY`, `RUVYXA_SHUTDOWN_GRACE`                                                                                                                            | จำนวนมิลลิวินาทีที่ยังรับ connection ต่อหลังรับสัญญาณ shutdown เพื่อให้ readiness probe อ่าน `503` ได้ และเวลาที่งานค้างมีให้ทำจนจบ            |
| `RUVYXA_PUBLIC_*`                                                                                                                                                        | browser-safe value ที่ inject เพื่อใช้ใน client                                                                                                |
| `RUVYXA_FUN`                                                                                                                                                             | ตั้งเป็น `0`/`false`/`off` เพื่อปิด spinner และมาสคอตที่วิ่งใน CLI โดยสีไม่เปลี่ยน                                                             |
| `RUVYXA_ASCII`                                                                                                                                                           | ตั้งเป็น `1` เพื่อวาด progress และ status ด้วย glyph แบบ ASCII เท่านั้น                                                                        |
| `FORCE_COLOR`, `CLICOLOR_FORCE`                                                                                                                                          | บังคับให้ output ที่ถูก redirect มีสี และกำหนดความลึกของสีได้: `1` = 16 สี, `2` = 256, `3` = 24-bit                                            |

output ของ CLI ยังเคารพ opt-out มาตรฐานของเทอร์มินัลสองตัว: `NO_COLOR` ปิดสี และ `TERM=dumb`
ปิดทั้งสี อนิเมชัน และ glyph ที่ไม่ใช่ ASCII ส่วน output ที่ถูก pipe หรือ redirect
จะไม่มีอนิเมชันเสมอ

`FORCE_COLOR` มีไว้สำหรับกรณีที่สองตัวนั้นตอบผิด: log ของ CI ที่ render ANSI ได้ มันมีลำดับเหนือทั้ง
`NO_COLOR` และ `TERM=dumb` เพราะมันเป็นตัวเดียวที่ถูกตั้งโดยตั้งใจสำหรับการรันครั้งนั้น
ไม่ใช่ค่าที่ติดมาจาก shell profile หรือ build image ส่วน `FORCE_COLOR=0` คือวิธีที่ variable
เดียวกันใช้ปฏิเสธ การบังคับสีไม่เคยบังคับอนิเมชัน เพราะ spinner ต้องวาดทับบนบรรทัดเดิม ซึ่ง log file
ทำไม่ได้

เมื่อเทอร์มินัลรายงานว่ารองรับสี 24-bit ส่วนที่เป็นการตกแต่งของ output — wordmark, เส้นคั่นใต้
header และหัวข้อ section, หางที่ลากตามมาสคอตใน progress, และแท่งขนาดใน `bench` — จะถูกวาดเป็น
gradient ส่วนสิ่งที่สื่อความหมายไม่ถูกวาดแบบนั้น: ทุก status, จำนวน, path และการจำแนกประเภท
ยังอยู่ในชุดสิบหกสีที่ทุกเทอร์มินัล render เหมือนกัน
ดังนั้นเทอร์มินัลที่มีสีน้อยกว่าจึงเสียแค่การตกแต่ง ไม่เคยเสียความแตกต่าง

variable ภายในที่ขึ้นต้นหรือลงท้ายด้วย double underscore เป็น runtime transport detail ไม่ใช่
application configuration ห้ามตั้งเอง ค่าเช่น `RUVYXA_AUTH_SECRET` ปรากฏใน auth scaffolder; ให้ใช้
private environment source และอย่าเปิดเผยด้วย public prefix

`RUVYXA_WORKER_MAX_QUEUE` มีค่าเริ่มต้นสี่เท่าของ `RUVYXA_WORKER_MAX_CONCURRENCY` มันจำกัด render
work ที่รออยู่ และคืน `RUV1705` เมื่อเต็ม ให้ใช้หลักฐานจาก load test ก่อนเพิ่มค่า เพราะ queue
ที่ใหญ่ขึ้นเก็บ request data มากขึ้นและเพิ่มเวลารอ

### กำหนด type ให้ public variable และเก็บ private variable ไว้ฝั่ง server

ประกาศ public variable ที่ client code อ่าน เพื่อให้ชื่อที่พิมพ์ผิดกลายเป็น TypeScript error
และทำให้ contract ที่ browser มองเห็นตรวจทานได้ โดยไม่ต้องเปิดเผยค่า private

```ts
// app/ruvyxa-env.d.ts
interface ImportMetaEnv {
  readonly RUVYXA_PUBLIC_APP_NAME: string
  readonly RUVYXA_PUBLIC_API_URL: string
}

interface ImportMeta {
  readonly env: ImportMetaEnv
}
```

```dotenv
# .env.example — commit เฉพาะชื่อและ placeholder ที่ไม่อ่อนไหว ห้ามใส่ secret จริง
RUVYXA_PUBLIC_APP_NAME=Example
RUVYXA_PUBLIC_API_URL=https://api.example.test
DATABASE_URL=replace-me-at-deploy
RUVYXA_AUTH_SECRET=replace-me-at-deploy
```

```tsx
// client component: ใช้ได้เฉพาะค่าที่เป็น public
'use client'
export function AppName() {
  return <span>{import.meta.env.RUVYXA_PUBLIC_APP_NAME}</span>
}
```

อย่าเพิ่ม `DATABASE_URL` หรือชื่อ private อื่นลงใน `ImportMetaEnv` และอย่าอ่านมันใน client component
ให้อ่านค่า private เฉพาะ server-only code เช่น loader, action หรือ API route การตรวจ boundary ของ
framework เป็นด่านเสริม ไม่ใช่เหตุผลให้วาง secret ใน shared module จับคู่ `.env.example` ที่ commit
ด้วย `requireEnv([...])` สำหรับชื่อที่ต้องมีจริงตอน release

### `cache.handler` — deployed build เก็บเอกสารที่ revalidate แล้วไว้ที่ไหน

route แบบ ISR หรือ PPR render ครั้งเดียวแล้วถูกเสิร์ฟจาก store จนหมด window ปกติแล้ว platform
เป็นคนตอบว่า store คืออะไร: Cloudflare Worker ได้ KV, serverless function
ได้ไดเรกทอรีเดียวที่เขียนได้, ส่วน `ruvyxa start` ใช้ build output ของตัวเอง

ไดเรกทอรีนั้นแยกต่อ instance และต่อ deployment สำหรับ container เดียวมันถูกแล้ว
แต่สำหรับแอปที่รันหลาย instance อยู่หลังโดเมนเดียวมันไม่ถูก: แต่ละ instance revalidate แยกกัน
แล้วผู้ชมได้สำเนาไหนก็แล้วแต่ load balancer เลือก มีแต่ตัวแอปที่รู้ว่าควรใช้ store ตัวไหนร่วมกัน
ตรงนี้คือที่ที่มันบอก

```ts
// ruvyxa.config.ts
export default {
  cache: { handler: './cache-handler.mjs' },
}
```

```js
// cache-handler.mjs — Redis, S3, ฐานข้อมูล อะไรก็ได้ที่ deployment มีอยู่แล้ว
export async function read(pathname, revalidate) {
  const entry = await store.get(pathname)
  if (!entry) return null // ไม่มีใน cache
  return { html: entry.html, stale: Date.now() - entry.storedAt >= revalidate * 1000 }
}

export async function write(pathname, html, revalidate, forced) {
  await store.set(pathname, { html, storedAt: Date.now() })
}
```

path เป็น project-relative และ module ถูก compile เข้าไปใน deployed bundle ดังนั้นมัน import
อะไรก็ได้ที่แอป import ได้ และมันไม่ถูกโหลดตอน build

export ทั้งสองตัวไม่บังคับ: ให้แค่ `read` ตัวเดียวก็ได้ platform ยังเขียนที่เดิมของมันอยู่
ถ้าไม่ประกาศอะไรเลย ทุก host ทำงานเหมือนเดิมทุกอย่าง

นี่คือ seam เดียวกับที่ Next.js เปิดไว้ในชื่อ `cacheHandler` ใน `next.config.js`
และมีอยู่ด้วยเหตุผลเดียวกัน — framework เลือก store ที่แอปใช้ร่วมกันแทนแอปไม่ได้

`revalidateTag()` เคลียร์ `cache()` ของ process นี้ทันทีเหมือนเดิมทุกอย่าง ถ้า handler export
`revalidateTag` ด้วย tag ที่ request นั้นสั่งไว้จะถูกส่งให้มัน หลังตอบ response —
ซึ่งคือสิ่งที่ทำให้การ invalidate ไปถึงทุก instance ไม่ใช่แค่ instance ที่รับ mutation นั้น:

```js
export async function revalidateTag(tags) {
  await store.dropEverythingLabelled(tags)
}
```

ไม่บังคับ และไม่มี fallback ของ platform: tag ติดป้ายอะไรก็ตามที่แอปตัดสินใจติด ส่วน cache ของ
platform ที่ key ด้วย URL ไม่มีอะไรให้ค้นด้วย tag ได้ โปรเจกต์ที่ไม่ประกาศ handler ทำงานเหมือนเดิม
คือ `revalidateTag()` เคลียร์ process เดียว

module เดียวกันนี้รองรับ `cache()` ได้ด้วย ซึ่งเป็นอีกครึ่งของสิ่งที่ Next.js วางไว้หลัง
`cacheHandler`:

```js
export async function readData(key) {
  const row = await store.get(key)
  // `populatedAt` คือเวลาที่ค่านั้นถูกผลิต ส่วนช่วงความสดถูกคำนวณใหม่จาก `ttl`
  // ที่โค้ดฝั่งเรียกขอมา entry ที่ instance อื่นเขียนไว้ด้วย ttl ยาวกว่า
  // จึงยืดอายุที่นี่ไม่ได้
  return row ? { value: row.value, populatedAt: row.populatedAt } : null
}

export async function writeData(key, entry) {
  await store.set(key, entry)
}
```

store ใน memory ของ process ยังตอบก่อนเสมอ — มันคือชั้นที่เร็ว และเป็นที่เดียวกับที่
`cacheMaxMemorySize` ของ Next.js อยู่ เฉพาะตอน miss ในเครื่องเท่านั้นที่ไปถาม shared store และเฉพาะ
miss ทั้งสองชั้นเท่านั้นที่ producer จะทำงาน ส่วนการเขียนถูกส่งออกไปโดย request ไม่ต้องรอ

store ที่ throw คือ cache ที่ช้าลง ไม่ใช่ request ที่ล้มเหลว: error ถูกรายงาน แล้ว producer ทำงานต่อ
ถ้าไม่ประกาศเลยไม่มีต้นทุนใดๆ — เส้นทางที่ไม่มี handler ไม่แม้แต่จะสร้าง promise

### `cache.maxEntries` — เก็บ `cache()` ไว้ใน process นี้เท่าไหร่

```ts
export default {
  cache: { handler: './cache-handler.mjs', maxEntries: 0 },
}
```

ชั้น in-memory เก็บ 1024 entry เป็นค่าเริ่มต้น เกินจากนั้นจะ evict ตัวที่ใช้ล่าสุดนานที่สุด ใส่ `0`
เพื่อปิดทิ้งทั้งชั้น ทุกการอ่านจะไปถึง shared store

นี่คือค่าที่ควรใช้เมื่อ deployment รันหลาย instance หลัง shared store เดียวกัน: สำเนาต่อ instance
ที่วางอยู่หน้า store ที่แชร์ คือสิ่งที่ทำให้สอง instance ตอบ key เดียวกันไม่เหมือนกัน
การปิดมันคือการแลก round trip หนึ่งครั้งกับคำตอบเดียว

Next.js เรียกการตัดสินใจเดียวกันนี้ว่า `cacheMaxMemorySize` และ `0` มีความหมายเดียวกัน
หน่วยต่างกันโดยตั้งใจ — store นี้นับเป็น entry และไม่มีการนับขนาดที่จะตอบ budget แบบไบต์ได้
ถ้าไปประมาณเอาก็จะได้ budget ที่ไม่มีใครเชื่อถือได้

ค่าที่ไม่ใช่จำนวนเต็มของ entry จะถูกรายงานแล้วเมิน: bound ที่ใช้ไม่ได้ต้องไม่กลายเป็น "ไม่มี cache"
หรือ "ไม่จำกัด" อย่างเงียบๆ ซึ่งเป็นสองทิศทางที่เจ็บ และหน้าตาเหมือน โค้ดที่ทำงานได้ทั้งคู่

### `cache.maxBytes` — ขอบเขตหน่วยความจำที่จำนวน entry บอกไม่ได้

```ts
export default {
  cache: { maxEntries: 1024, maxBytes: 52_428_800 },
}
```

`maxEntries` คุมว่าเก็บกี่ค่า แต่ไม่ได้บอกว่าแต่ละค่าใหญ่แค่ไหน หนึ่งพัน entry
ที่ค่าละสิบเมกะไบต์คือสิบกิกะไบต์ `maxBytes` ค่าเริ่มต้นห้าสิบเมกะไบต์ — เท่ากับที่ Next.js ตั้ง
`cacheMaxMemorySize` ไว้ — แล้ว evict ตัวที่ใช้ล่าสุด นานที่สุดจนกว่าจะพอดี

แต่ละค่าถูกชั่งจากความยาวหลัง serialize เป็นการประมาณ และเป็นตัวที่มีให้ใช้จริง: ทุกค่าที่ถูก cache
ผ่าน `assertCacheSerializable` มาแล้ว จึงชั่งแบบนี้ได้เสมอ
และการวัดที่คลาดไม่กี่เท่าดีกว่าไม่มีขอบเขตเลย ใส่ `0` เพื่อปิด byte budget แล้วให้ `maxEntries`
คุมตัวเดียว

ค่าที่ใหญ่กว่า budget ทั้งก้อนยังถูกเก็บหนึ่งครั้งแล้วถูก evict โดยการเขียนครั้งถัดไป
การเขียนที่รายงานว่าสำเร็จต้องไม่เหลืออะไรไว้ไม่ได้

### สิ่งที่ shared store ไม่ได้รับประกัน

สามข้อที่ควรรู้ก่อนชี้ `cache.handler` ไปที่ store บนเครือข่าย แต่ละข้อเป็นการ แลกที่ตั้งใจ
ไม่ใช่การมองข้าม:

- **การ invalidate รอ ส่วนการเติม cache ไม่รอ** `revalidateTag()` ถูก await ก่อนตอบ response เพราะ
  mutation ที่ตอบ `200` ได้บอกผู้เรียกไปแล้วว่าค่าเก่าหายไป การทำ write
  นั้นหายคือความผิดพลาดเชิงความถูกต้อง ส่วนการ _เขียน_ cache ไม่ถูก await เพราะการกัก response
  ไว้เพื่อเติม cache คือสิ่งตรงข้ามกับเหตุผลที่มี cache
- **write ที่ไม่รอ มีเพดาน** ค้างได้มากสุด 256 ตัว เกินจากนั้นถูกทิ้งและนับไว้
  ครั้งแรกที่ทิ้งถูกรายงาน ถ้าไม่มีเพดาน store ที่ช้าลงตอน traffic สูงจะสะสม promise
  หนึ่งตัวต่อหนึ่งค่าที่ผลิต แล้ว cache ที่มีไว้ปกป้อง origin จะกลายเป็นตัวที่ทำ process หมดแรงเอง
  ดูได้จาก `cacheStats()` ที่ `pendingSharedWrites` และ `droppedSharedWrites`
- **key พก build id ของ deployment นี้ไปด้วย** ไม่งั้นสอง deployment ที่ชี้ไป store เดียวกันจะเขียน
  `cache('user:1')` ทับกันและอ่านคำตอบของอีกฝั่ง handler ของคุณจะได้รับ key ที่เติม prefix แล้ว

**ก่อนหน้า:** [UI, navigation, metadata และ asset](06-ui-navigation-metadata-and-assets.md) ·
**ถัดไป:** [Plugin และ middleware](08-plugins-middleware.md)
