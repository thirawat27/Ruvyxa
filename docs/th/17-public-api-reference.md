# Public API reference

> **เป้าหมายของ tutorial:** เลือก public API ที่เล็กที่สุดสำหรับบทเรียนที่คุณกำลังทำ **เริ่มจาก:**
> ตัวอย่าง route และ data ในบท 4–9 **Checkpoint:** import จาก public entry point ที่ระบุไว้ แทน
> internal source path

reference นี้แสดง stable exported surface ที่พบใน package entry point โดยตั้งใจแยก implementation
detail ใน Rust/runtime file ออกจาก API ที่ application import

## `ruvyxa`, `ruvyxa/server` และ `ruvyxa/config`

คอลัมน์ **จาก** คือสิ่งที่ควรอ่านก่อน เพราะ entry point แต่ละตัวใช้แทนกันไม่ได้ `ruvyxa` re-export
primitive ที่โมดูลไหนก็ใช้ได้ ส่วนการเรียกที่ผูกกับ request — `cookies`, `headers`, `params`,
`draftMode` — มีเฉพาะบน `ruvyxa/server` และอ่านจาก store ที่ runtime ติดตั้งไว้รอบ render หรือ
handler การเรียกที่ระดับ module scope หรือจากโค้ดฝั่ง browser จะ throw
`… was called outside a request` แทนที่จะคืนค่าว่าง path ที่ import
จึงเป็นคำอธิบายที่ชัดที่สุดว่าโค้ดนั้นตั้งใจให้รันที่ไหน

| Export                                          | จาก                           | Signature / วัตถุประสงค์                                                                                                                                                                                    |
| ----------------------------------------------- | ----------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `config`                                        | `ruvyxa/config` หรือ `ruvyxa` | `<T extends RuvyxaConfig>(config: T) => T`; typed config identity helper                                                                                                                                    |
| `loader`                                        | ทั้งสอง                       | `(handler: LoaderHandler<T>) => Loader<T>`; handler รับ `params`, `request`, `cache`                                                                                                                        |
| `action`                                        | ทั้งสอง                       | Builder: `.input(schema)`, `.realtime(channels?)`, `.handler(fn)`                                                                                                                                           |
| `cache`                                         | ทั้งสอง                       | `(key) => CacheBuilder`; `.ttl`, `.swr`, `.tags(...keys)`, `.scope(...)` และ `.get(...)`                                                                                                                    |
| `invalidateCache`, `cacheStats`                 | ทั้งสอง                       | ลบ cache entry แบบ exact/prefix/all; รายงาน `{ size, maxEntries }`                                                                                                                                          |
| `pruneCache`                                    | ทั้งสอง                       | `() => number`; ลบ entry ที่หมดอายุเต็มที่แล้วทั้งหมดและรายงานจำนวนที่ลบ เป็น sweep ที่โมดูลรันเองทุก 60 วินาที entry ที่พ้นช่วง stale แล้วเสิร์ฟให้ใครไม่ได้อีก การลบจึงคืนหน่วยความจำโดยไม่เปลี่ยนคำตอบใด |
| `revalidateTag`                                 | `ruvyxa/server`               | `(tag: string) => void`; ลบทุก cache entry ที่มี tag นั้นแบบตรงตัว                                                                                                                                          |
| `json`, `redirect`, `status`                    | ทั้งสอง                       | Response helper · `redirect` อนุญาตเฉพาะ 3xx · `status(code, message?)` สร้าง response ได้ทุก 200–599 และปฏิเสธ body บน 204/205/304 · `notFound()` ที่ throw เป็นของ `@ruvyxa/react`                        |
| `cookies`, `headers`, `draftMode`               | `ruvyxa/server`               | อ่าน request ที่กำลังให้บริการ เรียกตัวใดตัวหนึ่งแล้วจะกัน render นี้ออกจาก cache ที่ใช้ร่วมกัน                                                                                                             |
| `params`                                        | `ruvyxa/server`               | route parameter ของหน้าที่กำลัง render อ่านได้จากใต้ component ที่รับ props มาแล้ว                                                                                                                          |
| `revalidatePath`                                | `ruvyxa/server`               | `(path: string) => void`; คิว URL จริงหนึ่งอันให้ render ใหม่ในคำขอถัดไป                                                                                                                                    |
| `FlightContext`, `FlightHandler`, `FlightValue` | `ruvyxa/server`               | type สำหรับ route export `flight` แบบ public และ payload ที่มันคืน                                                                                                                                          |
| `definePlugin`, `withResponseHeader`            | `ruvyxa/plugin` หรือ `ruvyxa` | plugin definition และ response-header helper                                                                                                                                                                |
| `standaloneServerSource`                        | `ruvyxa`                      | source generator สำหรับ standalone server artifact                                                                                                                                                          |

"ทั้งสอง" หมายถึงชื่อนั้น re-export ทั้งจาก `ruvyxa` และ `ruvyxa/server` ในโมดูลที่รันฝั่ง server
อย่างเดียว ควรเลือก `ruvyxa/server` เพื่อให้ตัว import เองบอกขอบเขตไว้

ผู้เขียน adapter ยังได้ build helper จาก `ruvyxa` ด้วย แบ่งเป็นสี่กลุ่ม

| กลุ่ม                      | ชื่อ                                                                                                                                                                                                                                                   |
| -------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Build context และ out dir  | `validateBuildContext`, `clientBuildOutput`, `runtimeBuildPolicy`, `projectRelativeOutDir`, `assertSafeOutDirForCommand`                                                                                                                               |
| Asset และ header           | `staticAssetGlobs`, `publicAssetGlobs`, `staticAssetPattern`, `headersFileContents`, `DEFAULT_SECURITY_HEADERS`, `IMMUTABLE_CACHE_CONTROL`, `PUBLIC_ASSET_CACHE_CONTROL`, `STATIC_ASSET_EXTENSIONS`, `CLIENT_BUNDLE_PREFIX`, `DEFAULT_IMAGE_MAX_WIDTH` |
| Deploy manifest            | `parseDeployManifest`, `deployHeaderRules`, `documentCacheControl`, `routeServeMode`, `nonPublishableStrategies`, `DEPLOY_MANIFEST_KEY`, `DEPLOY_MANIFEST_VERSION`, `DOCUMENT_CACHE_CONTROL`                                                           |
| Document store ที่ถูก emit | `platformDocumentStoreSource`, `documentCacheOptionsSource`, `isrTemporaryCacheSource`, `isrTemporaryCacheDirSource`                                                                                                                                   |

แถวสุดท้ายคือ source ที่ adapter ปล่อยลงใน handler ที่มันสร้าง ไม่ใช่สิ่งที่เรียกตอน build:
`platformDocumentStoreSource` คืน ISR/PPR document store ที่ wrapper ของแต่ละแพลตฟอร์มติดตั้ง
adapter ทั้งสิบเอ็ดตัวจึงไม่ต้องถือสำเนาคนละชุด ดูการใช้งานจริงได้ที่
[คู่มือ adapter สำหรับแพลตฟอร์ม](20-platform-adapter-guide.md)

type มี `RuvyxaConfig`, `PageProps`, `GetStaticParams`, `RenderStrategy`, `Adapter`,
`AdapterInspection`, `MiddlewareConfig`, `ImageConfig`, `I18nConfig`, `SiteConfig`, subtype ของ site
คือ `SiteSitemapConfig`, `SiteSitemapEntry`, `SiteSitemapEntryDefaults`, `SiteSitemapVideo`,
`SiteRobotsConfig` และ `SiteRobotsRule`, subtype ของ content คือ `ContentConfig` และ
`ContentEngineConfig`, type ของ deploy manifest คือ `DeployManifest`, `DeployRoute` และ
`DeployServeMode` และ plugin contract ใช้ import จาก `ruvyxa` สำหรับ public primitive และ
`ruvyxa/config` หรือ `ruvyxa/plugin` เพื่อสื่อ intent ชัดเจน

## `@ruvyxa/react`

| Export family         | ชื่อหลัก                                                                                                                               |
| --------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| Navigation            | `Link`, `RouteContext`, `useRouter`, `usePathname`, `useParams`, `useSearchParams`, `useSelectedRoute`, `useRouteContext`, `useFlight` |
| Rendering error       | `RuvyxaErrorBoundary`, `notFound`, `isNotFoundError`, `RouteErrorProps`                                                                |
| Metadata/content      | `Seo`, `Meta`, `MetaFactory`, `Answer`                                                                                                 |
| Browser/runtime       | `hydrate`, `reportHydrationError`, `useRuvyxaLoader`                                                                                   |
| Asset                 | `Image`, `Picture`, `Script`, `DEFAULT_DEVICE_WIDTHS`                                                                                  |
| Typed routes          | `route`, `RouteHref`, `RoutePattern`, `KnownRoute`, `RuvyxaRouteRegistry`                                                              |
| Low-level integration | `getRouterInstance`, `resetInjectedScripts`, `NOT_FOUND_PROPERTY`                                                                      |

`useRuvyxaLoader<T>(loader, { enabled?, deps? })` คืน `{ data, loading, error, refetch }`
`hydrate({ root?, onError? })` dispatch hydration event และติดตั้ง reporter ที่ entry
ที่สร้างขึ้นส่ง `onRecoverableError`, `onCaughtError` และ `onUncaughtError` ของ React ให้; report
ที่เกิดก่อนติดตั้งจะถูก เข้าคิวและส่งให้ตอนติดตั้ง และ `context.kind` บอกว่ามาจาก callback ไหน
`notFound()` จาก package นี้ throw เสมอ จึงคืน `never` `<Script strategy>` มีค่าเป็น
`beforeInteractive`, `afterInteractive` (ปริยาย) หรือ `lazyOnload` `RouteHref` เป็น `string`
เว้นแต่เปิด `typedRoutes` และไฟล์ declaration ที่ generate อยู่ใน `include` ของ tsconfig;
`route(href)` assert string ตอน runtime ให้เป็นชนิดนี้

`useFlight<T>()` อ่าน public payload จาก soft navigation ปัจจุบัน และคืน `undefined` เมื่อ route ที่
match ไม่มี `flight` export หรือ document แรกที่ SSR ไม่ได้มี inline payload `getRouterInstance`,
`resetInjectedScripts` และ `NOT_FOUND_PROPERTY` เป็น seam ระดับ low-level สำหรับ integration/test
โดยปกติ application ควรใช้ hook และ component ด้านบน

## `@ruvyxa/core/route-match`

route matcher ที่ทุก JavaScript host ใช้ร่วมกัน — browser router, serverless handler และ standalone
server resolve URL ผ่าน module เดียวกันนี้ การคลิก link กับการ reload URL เดิมจึงให้ผลต่างกันไม่ได้

| Export                                            | วัตถุประสงค์                                                           |
| ------------------------------------------------- | ---------------------------------------------------------------------- |
| `createRouteMatcher(routes)`                      | compile route table ครั้งเดียว; คืน `(pathname) => RouteMatch \| null` |
| `canonicalRoutePath(pathname)`                    | decode path หนึ่งครั้งเป็น canonical segment หรือ `null` ถ้าต้องปฏิเสธ |
| `compilePattern`, `routeSpecificity`              | compile pattern และลำดับ static มาก่อน dynamic                         |
| `compareSpecificity`, `normalizeMatchPath`        | primitive สำหรับการจัดลำดับและ normalize slash                         |
| `bindPatternParams(pattern, matched)`             | ผูก capture ของ compiled pattern เข้ากับ parameter ที่มีชื่อ           |
| `RouteParams`, `RouteMatch`, `RouteManifestEntry` | type ของการ match                                                      |

ปกติ application ต้องการแค่ `useParams()` จาก `@ruvyxa/react` ส่วนเหล่านี้มีไว้สำหรับโค้ดที่ต้อง
resolve route นอก React tree เช่น custom server หรือ adapter

> `@ruvyxa/react` export type `RouteParams` ส่วนชื่ออื่นทั้งหมดในตารางนี้มาจาก
> `@ruvyxa/core/route-match` และมาจากที่นั่นที่เดียว

## Public package อื่น

| Package             | Integration ที่ export                                                                                                                          |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------- |
| `@ruvyxa/auth`      | `createAuth`, provider, store, client/plugin entry point, auth type/error และ `forwardedClientIp(request)` สำหรับ rate limit หลัง trusted proxy |
| `@ruvyxa/database`  | `createDatabase`, operation/type, `prismaAdapter`, `dynamoAdapter`, `defineDatabaseAdapter`                                                     |
| `@ruvyxa/realtime`  | plugin entry point; client export `createRealtimeClient`                                                                                        |
| `@ruvyxa/testing`   | `mockLoader`, `mockAction`, `mockCache`                                                                                                         |
| `@ruvyxa/adapter-*` | typed build adapter package                                                                                                                     |

สำหรับ option/default detail ให้ดู [Configuration](07-configuration.md) และ exported TypeScript
declaration ใน package ที่ติดตั้ง ชื่อ public API ที่แสดงถูกยืนยันจาก source; runtime name
ที่ขึ้นต้น `RUVYXA_` และมี double underscore ไม่ใช่ application API

**ก่อนหน้า:** [Troubleshooting และ compatibility เมื่ออัปเกรด](16-troubleshooting-upgrades.md) ·
**ถัดไป:** [ขอบเขตเอกสารและแหล่งข้อมูล](18-documentation-scope-and-sources.md)
