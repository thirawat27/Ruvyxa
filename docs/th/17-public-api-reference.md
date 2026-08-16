# Public API reference

> **เป้าหมายของ tutorial:** เลือก public API ที่เล็กที่สุดสำหรับบทเรียนที่คุณกำลังทำ **เริ่มจาก:**
> ตัวอย่าง route และ data ในบท 4–9 **Checkpoint:** import จาก public entry point ที่ระบุไว้ แทน
> internal source path

reference นี้แสดง stable exported surface ที่พบใน package entry point โดยตั้งใจแยก implementation
detail ใน Rust/runtime file ออกจาก API ที่ application import

## `ruvyxa`, `ruvyxa/server` และ `ruvyxa/config`

| Export                                          | Signature / วัตถุประสงค์                                                                        |
| ----------------------------------------------- | ----------------------------------------------------------------------------------------------- |
| `config`                                        | `<T extends RuvyxaConfig>(config: T) => T`; typed config identity helper                        |
| `loader`                                        | `(handler: LoaderHandler<T>) => Loader<T>`; handler รับ `params`, `request`, `cache`            |
| `action`                                        | Builder: `.input(schema)`, `.realtime(channels?)`, `.handler(fn)`                               |
| `cache`                                         | `(key) => CacheBuilder`; `.ttl`, `.swr`, `.tags(...keys)`, `.scope(...)` และ `.get(...)`        |
| `invalidateCache`, `cacheStats`                 | ลบ cache entry แบบ exact/prefix/all; รายงาน `{ size, maxEntries }`                              |
| `FlightContext`, `FlightHandler`, `FlightValue` | type สำหรับ route export `flight` แบบ public และ payload ที่มันคืน                              |
| `json`, `redirect`, `notFound`                  | Response helper; redirect อนุญาตเฉพาะ status 3xx                                                |
| `cookies`, `headers`, `draftMode`               | อ่าน request ที่กำลังให้บริการ เรียกตัวใดตัวหนึ่งแล้วจะกัน render นี้ออกจาก cache ที่ใช้ร่วมกัน |
| `revalidatePath`                                | `(path: string) => void`; คิว URL จริงหนึ่งอันให้ render ใหม่ในคำขอถัดไป                        |
| `definePlugin`, `withResponseHeader`            | plugin definition และ response-header helper                                                    |
| `standaloneServerSource`                        | source generator สำหรับ standalone server artifact                                              |

type มี `RuvyxaConfig`, `PageProps`, `GetStaticParams`, `RenderStrategy`, `Adapter`,
`MiddlewareConfig`, `ImageConfig`, `I18nConfig`, `SiteConfig` และ plugin contract ใช้ import จาก
`ruvyxa` สำหรับ public primitive และ `ruvyxa/config` หรือ `ruvyxa/plugin` เพื่อสื่อ intent ชัดเจน

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
`hydrate({ root?, onError? })` dispatch hydration event และติดตั้ง reporting ที่เลือกได้
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

> **เปลี่ยนใน 1.0.28** ชื่อเหล่านี้เคย re-export จาก `@ruvyxa/react` และตอนนี้ไม่แล้ว
> `@ruvyxa/react` ยัง export type `RouteParams` อยู่ ส่วนที่เหลือให้ import จาก
> `@ruvyxa/core/route-match`

## Public package อื่น

| Package             | Integration ที่ export                                                                      |
| ------------------- | ------------------------------------------------------------------------------------------- |
| `@ruvyxa/auth`      | `createAuth`, provider, store, client/plugin entry point, auth type/error                   |
| `@ruvyxa/database`  | `createDatabase`, operation/type, `prismaAdapter`, `dynamoAdapter`, `defineDatabaseAdapter` |
| `@ruvyxa/realtime`  | plugin entry point; client export `createRealtimeClient`                                    |
| `@ruvyxa/testing`   | `mockLoader`, `mockAction`, `mockCache`                                                     |
| `@ruvyxa/adapter-*` | typed build adapter package                                                                 |

สำหรับ option/default detail ให้ดู [Configuration](07-configuration.md) และ exported TypeScript
declaration ใน package ที่ติดตั้ง ชื่อ public API ที่แสดงถูกยืนยันจาก source; runtime name
ที่ขึ้นต้น `RUVYXA_` และมี double underscore ไม่ใช่ application API

**ก่อนหน้า:** [Troubleshooting และ compatibility เมื่ออัปเกรด](16-troubleshooting-upgrades.md) ·
**ถัดไป:** [ขอบเขตเอกสารและแหล่งข้อมูล](18-documentation-scope-and-sources.md)
