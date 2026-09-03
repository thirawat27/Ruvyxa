# ข้อมูล, action และ API route

> **เป้าหมายของ tutorial:** ย้ายการอ่านข้อมูลและการเขียนไปฝั่ง server แล้วเปิด HTTP API
> ขนาดเล็กอย่างปลอดภัย **เริ่มจาก:** route ที่ render ถูกต้องจาก
> [Routing และ rendering](04-routing-rendering.md) **Checkpoint:** ทดสอบ loader, action หรือ API
> route อย่างน้อยหนึ่งรายการด้วย input ที่ถูกและผิด

## Loader และ in-memory cache

`loader(handler)` สร้าง async callable ที่ติดเครื่องหมายเป็น Ruvyxa loader handler รับ
`{ params, request, cache }` `cache(key)` คือ cache ภายใน process ที่จำกัด LRU 1024 entry, TTL
ปริยาย 60 วินาที, รองรับ stale-while-revalidate และ prefix invalidation มันไม่ใช่ distributed cache

```ts
// app/products/server.ts
import { cache, loader } from 'ruvyxa/server'

export const products = loader(async ({ cache }) =>
  cache('products:list')
    .ttl('5m')
    .swr('1m')
    .get(async () => {
      const response = await fetch('https://example.test/products')
      if (!response.ok) throw new Error(`Upstream returned ${response.status}`)
      return response.json()
    }),
)
```

ระยะเวลา cache รับจำนวนเต็มบวกตามด้วย `ms`, `s`, `m`, `h` หรือ `d` `invalidateCache('products')` ลบ
`products` และ key ที่ขึ้นต้นด้วย `products:`; หากไม่ส่ง argument จะล้าง cache ทั้ง process เรียก
`cacheStats()` เพื่อได้ `{ size, maxEntries }`

`.scope('request')` จะเก็บค่าไว้เฉพาะ request ปัจจุบันแทนการใช้ร่วมกันข้าม request ใช้เมื่อ producer
อ่าน cookie, header หรือ draft mode — producer ที่ใช้ร่วมกันแล้วไปอ่าน request state จะ fail closed
แทนที่จะรั่วข้อมูลของผู้ใช้คนหนึ่งไปให้อีกคน

### `scope('deployment')` หมายถึง process ไม่ใช่ deployment

scope ค่าเริ่มต้นใช้ค่าร่วมกันกับทุก request ที่ **process เดียวกัน** รับผิดชอบ ไม่ได้ใช้ร่วมกันทั้ง
deployment และความต่างนี้จะมองไม่เห็นเลยจนกว่าคุณจะรันมากกว่าหนึ่ง instance:

| รันที่ไหน                                     | ค่าที่ cache ไว้หนึ่งค่ามีต้นทุนเท่าไร                                                                                         |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------ |
| `ruvyxa start` บนเครื่องเดียว                 | คำนวณหนึ่งครั้ง **ต่อ render worker หนึ่งตัว** เพราะเซิร์ฟเวอร์คือ pool ของ process ที่ปรับขนาดตามเครื่อง ไม่ใช่ process เดียว |
| สอง instance หลัง load balancer               | เท่ากับ pool ข้างบนคูณสอง                                                                                                      |
| Serverless (Lambda, Cloud Functions, Workers) | คำนวณหนึ่งครั้งต่อ container ที่อุ่นอยู่ คำนวณใหม่ทุก cold start และแยกกันในทุก container ที่ทำงานพร้อมกัน                     |

ชื่อ scope บอกว่าค่าจะถือว่าใช้ได้นานแค่ไหน ไม่ได้บอกว่ากี่เครื่องเห็นค่านั้น นี่คือขอบเขตที่
`invalidateCache()` ทำงาน — ล้าง cache ของ process ที่เรียกเท่านั้น ค่าที่ cache ไว้ที่อื่นจะอยู่ต่อ
จนกว่า TTL ของมันเองจะหมด ส่วน `revalidateTag()` ไปได้ไกลกว่านั้นเมื่อโปรเจกต์ประกาศ
[`cache.handler`](07-configuration.md) — ดู
[invalidate ด้วย tag แทน key](#invalidate-ด้วย-tag-แทน-key)

นั่นทำให้ `cache()` เหมาะกับสิ่งที่มันเป็น — ตัว memoize ภายใน process ที่ดูดซับงานซ้ำ ๆ
ภายในเซิร์ฟเวอร์ตัวเดียว — และไม่เหมาะจะเป็นที่เก็บค่าเพียงที่เดียว
ค่าที่ต้องเหมือนกันทุกที่ควรอยู่ในที่เก็บที่ตั้งใจให้ใช้ร่วมกัน (Redis, KV, ฐานข้อมูลของคุณ) แล้วใช้
`cache()` ครอบตอนอ่าน

### invalidate ด้วย tag แทน key

key ระบุ entry เดียว ส่วน tag กำกับกลุ่มของ entry ทำให้ key
ที่ไม่เกี่ยวข้องกันแต่หมดอายุพร้อมกันถูกล้างได้ในครั้งเดียว

```ts
import { cache, revalidateTag } from 'ruvyxa/server'

const list = await cache('products:list').tags('products').get(loadList)
const featured = await cache('home:featured').tags('products', 'home').get(loadFeatured)

revalidateTag('products') // ลบทั้ง `products:list` และ `home:featured`
```

`revalidateTag(tag)` ลบทุก cache entry ที่มี tag นั้นแบบตรงตัว — การ match เป็นแบบ exact ไม่มีรูป
prefix หรือ wildcard และแต่ละ tag ยาว 1–128 ตัวอักษร ประกอบด้วยตัวอักษร ตัวเลข `:`, `.`, `_`, `/`
หรือ `-` เท่านั้น นอกเหนือจากนี้จะ throw

ขอบเขตที่มันไปถึงขึ้นกับว่าโปรเจกต์ประกาศ [`cache.handler`](07-configuration.md) ไว้หรือไม่ ถ้าไม่มี
handler มันทำงานกับ cache ของ process ที่เรียกมัน บน serverless จึงล้างเฉพาะ instance ที่เรียก
ไม่ใช่ instance อื่นที่อุ่นอยู่แล้ว ถ้ามี handler ที่ export `revalidateTag` tag จะถูกส่งให้ store
นั้นหลังตอบ response ด้วย การอ่านครั้งถัดไปของทุก instance จึง miss ที่ store เช่นกัน — แต่ละ
instance ยังตอบจากสำเนาใน memory ของตัวเองจนกว่า entry นั้นจะหมดอายุ ซึ่งเป็นเหตุผลที่มี
`cache.maxEntries: 0`

`invalidateCache()` ไม่มีครึ่งหลังนี้ เพราะ contract ของ handler ไม่มีการ invalidate ระดับ key
คีย์ที่ล้างตรงนี้จึงยังอยู่ใน shared store และการอ่านครั้งถัดไปบน instance นี้จะอ่านมันกลับมา
สิ่งที่ต้อง invalidate ผ่าน shared store ให้ติด tag ไว้

มันล้าง **ค่า** ที่ cache ไว้ ไม่ใช่ HTML ที่ pre-render แล้ว หากต้องการให้ server render
เอกสารที่เก็บไว้ใหม่ ให้ใช้ [`revalidatePath()`](#revalidate-ตามคำสั่ง)

## Public Flight payload

> **ไม่ใช่ React Server Components** Flight ของ Ruvyxa เป็นคนละอย่าง: เป็น JSON payload ต่อ route
> ที่ page เลือกเปิดใช้เอง และถูก fetch ระหว่าง soft navigation ไม่ใช่ wire format ของ React Flight,
> `flight` ไม่ใช่ server component และโมดูล `'use client'` ไม่ได้ถูกแปลงเป็น client reference โดย
> export นี้ Ruvyxa มี React Server Components จริง แต่อยู่หลัง opt-in คนละตัวที่ไม่มีอะไรร่วมกับ
> export นี้ — ดู [React Server Components](04-routing-rendering.md#react-server-components) และ
> route หนึ่งใช้ทั้งสองอย่างพร้อมกันไม่ได้: page ที่ใช้ server components ไม่มี `flight` export
> ให้เรียก เพราะข้อมูลของมันอยู่ใน payload ของ React อยู่แล้ว

page สามารถ export `flight` เพื่อส่งข้อมูลสาธารณะที่ผูกกับ artifact version ได้ function นี้รับเฉพาะ
canonical path และ route params และต้องคืนข้อมูลที่ serialize เป็น JSON ได้ Client component อ่าน
payload ของ route ปัจจุบันผ่าน `useFlight<T>()` จาก `@ruvyxa/react` เมื่อ request ล้มเหลวหรือ
artifact version ไม่ตรงกัน browser จะ fallback ไป navigation แบบเต็มหน้า

```ts
// app/products/[id]/page.tsx
import type { FlightHandler } from 'ruvyxa/server'

export const flight: FlightHandler = async ({ params }) => ({
  productId: params.id,
  summary: 'Public product details',
})
```

ใส่ module directive `'use cache'` ไว้บรรทัดต้นเมื่อ public payload นี้ cache ได้ directive
นี้ต้องมี `flight` export, ใช้ bounded cache ของ Ruvyxa กับ key จาก route และ params แบบ
deterministic และใช้ กับ static-only adapter ไม่ได้ การอ่าน private request state จาก producer ที่
cache ไว้จะ fail closed; ข้อมูล authenticated ควรใช้ API route หรือ server action

endpoint `/__ruvyxa/flight` เป็น transport ภายในและปฏิเสธ request ที่มี cookie หรือ authorization
production build สร้าง manifest ชื่อสั้น `references.json`, `actions.json` และ `flight.json` โดยชื่อ
contract คงที่ ส่วน compatibility number แยกอยู่ใน `schemaVersion`, `protocolVersion` และ
`artifactVersion`

## Server action

สร้าง action ด้วย `action.input(schema).handler(handler)` schema ต้องมี synchronous `parse(value)`
action handler รับ `input` ที่ parse แล้ว, request, user data (หากมี) และ `invalidate(key)`
`.realtime(channels?)` จะ publish หลังเรียกสำเร็จเมื่อ realtime capability ถูกตั้งค่า

```ts
// app/todos/action.ts
import { action } from 'ruvyxa/server'

export const createTodo = action
  .input({
    parse(value: unknown) {
      if (!value || typeof value !== 'object' || !('title' in value))
        throw new Error('title required')
      return { title: String(value.title).trim() }
    },
  })
  .realtime('todos')
  .handler(async ({ input, invalidate }) => {
    if (!input.title) throw new Error('title required')
    invalidate('todos')
    return { id: crypto.randomUUID(), ...input, completed: false }
  })
```

action รับ realtime channel ได้สูงสุด 16 ช่อง ชื่อ channel ใช้ตัวอักษร, ตัวเลข, `:`, `.`, `_`, `/`
หรือ `-` ความยาว 1–128 กำหนด payload และ rate restriction ของ action ใต้ `security`; ดู
[Security](13-security.md)

## API route

วาง `route.ts` ใน folder เป้าหมาย และ export HTTP method function ตัวพิมพ์ใหญ่
`app/api/echo/route.ts` ใน demo export `POST({ request })`, อ่าน JSON และคืน `Response.json` ใช้
response helper มาตรฐานได้: `json(data, init)`, `redirect(location, status)` และ
`status(code, message?)` จาก `ruvyxa/server` เช่น `status(404)` หรือ `status(403, 'Forbidden')`
มันรับ code เข้ามา ทุก status จึงมี helper และที่ไม่ได้ชื่อ `notFound` เพราะ `@ruvyxa/react` มี
`notFound()` ที่ throw เพื่อ render `not-found.tsx` แทน

```ts
// app/api/health/route.ts
export function GET() {
  return Response.json({ ok: true })
}
```

handler จะคืนค่าข้อมูลแทน `Response` ก็ได้ ค่าที่ไม่ใช่ `Response` จะถูกส่งเป็น
`Response.json(value)` ดังนั้น `return { ok: true }` กับ `return Response.json({ ok: true })`
ให้ผลเหมือนกัน — รวมถึง string เปล่า ๆ ที่จะถูก encode เป็น JSON และตอบด้วย `application/json`
มีสองกรณีที่หยุด request ด้วย `RUV1504` แทน คือไม่คืนค่าอะไรเลย และคืนค่าที่ JSON serialise ไม่ได้
ให้คืน `Response` เมื่อ status, header หรือ content type มีความสำคัญ

route handler ต้อง validate body ที่ไม่น่าเชื่อถือก่อนใช้ API payload limit อยู่ที่
`security.apiLimit`; action payload ใช้ `security.actionLimit`

**ก่อนหน้า:** [Routing และ rendering](04-routing-rendering.md) · **ถัดไป:**
[UI, navigation, metadata และ asset](06-ui-navigation-metadata-and-assets.md)

## อ่านข้อมูลจาก request

`cookies()`, `headers()` และ `draftMode()` อ่าน request ที่กำลังถูกให้บริการอยู่ ใช้ได้ทั้งใน page
component, API route handler และ server action ไม่ต้องส่งพารามิเตอร์ผ่านลงไปเอง: runtime ติดตั้ง
store ต่อ request ครอบ render และ handler ไว้ให้ แล้วฟังก์ชันเหล่านี้อ่านจากตรงนั้น

```tsx
// app/dashboard/page.tsx
import { cookies, draftMode, headers } from 'ruvyxa/server'

export default function Dashboard() {
  const theme = cookies().get('theme') ?? 'light'
  const locale = headers().get('accept-language') ?? 'en'
  if (draftMode().isEnabled) return <DraftPreview locale={locale} />
  return <main data-theme={theme} lang={locale} />
}
```

- `cookies()` คืน `{ get, has, getAll }` ที่อ่านจากเฮดเดอร์ `Cookie` ค่าที่ได้เป็นค่าที่ส่งมาจริง
  ตัดเพียงช่องว่างหัวท้ายและเครื่องหมายคำพูดหนึ่งชั้น ส่วนการ percent-decode เป็นหน้าที่ของคุณ
  เพราะไม่ใช่ทุก cookie ที่เข้ารหัสไว้ และการ decode ค่าที่ไม่ได้เข้ารหัสจะ throw
- `headers()` คืน `Headers` มาตรฐานแบบอ่านอย่างเดียว `get`, `has` และการวนลูปทำงานเหมือนบน `Request`
- `draftMode()` บอกว่ามี cookie `__ruvyxa_draft` อยู่หรือไม่ ให้ตั้งค่าจาก API route หลังตรวจ secret
  ที่ CMS ของคุณใช้ร่วมกับแอปแล้ว

### การเรียกเหล่านี้เปลี่ยนวิธี cache หน้า

การเรียกฟังก์ชันใดก็ตามข้างต้นเป็นการบอก Ruvyxa ว่า HTML ที่ได้เป็นของผู้เข้าชมคนเดียว เอกสารนั้นจะ
ไม่ถูกเก็บใน render cache และถ้าเป็นกลยุทธ์แบบ prerender ก็จะไม่ถูกเขียนลง ISR cache ด้วย
ไม่ต้องประกาศอะไรและไม่ต้อง export อะไร — ตัวการเรียกคือการประกาศในตัวมันเอง

ผลที่ควรรู้: หน้าที่อ่าน request จะ render ใหม่ทุกครั้ง ถ้าคุณต้องการ cookie แค่กับส่วนเล็ก ๆ
ของหน้า การย้ายส่วนนั้นไปไว้ใน island ที่มี `'use client'` จะทำให้ส่วนที่เหลือยัง cache ได้

### อ่าน route parameter จากจุดไหนก็ได้ในหน้า

`params()` คืนค่า route parameter ที่ match กับหน้าที่กำลัง render อยู่ ตัว page เองได้รับค่านี้เป็น
props อยู่แล้ว — `params()` มีไว้สำหรับทุกอย่างที่อยู่ _ใต้_ มันลงไป

```tsx
// app/[lang]/blog/[slug]/page.tsx
import { params } from 'ruvyxa/server'

function PublishedOn({ date }: { date: Date }) {
  const { lang } = params()
  return <time dateTime={date.toISOString()}>{date.toLocaleDateString(lang as string)}</time>
}

export default function Post() {
  const { slug } = params()
  return (
    <article>
      <PublishedOn date={publishedAt(slug as string)} />
    </article>
  )
}
```

ต่างจากสามฟังก์ชันข้างบน — **ตัวนี้ไม่เปลี่ยนวิธี cache ของหน้า** เพราะ parameter เป็นส่วนหนึ่งของ
ตัวตนของ route ไม่ใช่ของคนที่เรียก: `/th/blog/hello` render ออกมาเป็นเอกสารเดียวกันสำหรับทุกคน
หน้าที่อ่าน params ของตัวเองจึงยังคง render แบบ static ได้และยังเก็บใน ISR cache ได้ตามเดิม

- segment ที่ประกาศเป็น catch-all จะได้ค่าเป็น array ตรงตามที่ตัว matcher สร้างไว้
- parameter ที่ไม่มีอยู่ใน route จะเป็น `undefined` เหมือนการอ่าน key ที่ไม่มี
- ใช้ได้ทั้งใน page และใน API route handler ส่วน server action ถูกเรียกที่ endpoint ของตัวเอง ไม่ได้
  match กับ route pattern จึงไม่มี route parameter — `params()` จะแจ้งตรง ๆ แทนที่จะคืน object ว่าง
  ไม่งั้นการพิมพ์ชื่อ segment ผิดจะดูเหมือน "route นี้ไม่มี parameter ตัวนั้น"

ถ้าอยู่ใน component ที่เป็น `'use client'` ให้ใช้ `useParams()` จาก `@ruvyxa/react` แทน เพราะฝั่ง
เบราว์เซอร์ไม่มี per-request store ให้อ่าน

### เรียกนอก request

ทั้งสองกรณีจะ throw พร้อมข้อความที่ระบุชื่อฟังก์ชัน:

- **ที่ระดับ module** โค้ดระดับ module ทำงานตอน import ซึ่งยังไม่มี request ให้ย้ายการเรียกเข้าไป ใน
  component หรือ handler
- **ระหว่าง ISR revalidation เบื้องหลัง** การ re-render ตามกำหนดเวลาไม่มีผู้เข้าชม นี่เป็นเจตนา:
  ทางเลือกอีกทางคือได้หน้าที่สร้างจาก session ของ "ไม่มีใคร" แล้วเอาไป cache ให้ทุกคน

## Revalidate ตามคำสั่ง

`revalidatePath(path)` สั่งให้เซิร์ฟเวอร์ render URL หนึ่งใหม่ในคำขอถัดไปที่สำเร็จ เรียกได้จาก API
route หรือ server action — คำสั่งจะเดินทางกลับไปพร้อมกับ response ของ handler นั้น ดังนั้น client
ที่นำทางต่อ หลังได้ผลสำเร็จจะไม่มีทางมาถึงก่อนที่ cache จะถูกล้าง

```ts
// app/api/revalidate/route.ts
import { revalidatePath } from 'ruvyxa/server'

export async function POST({ request }: { request: Request }) {
  const { path } = await request.json()
  revalidatePath(path)
  return Response.json({ revalidated: path })
}
```

อาร์กิวเมนต์คือ URL จริง (`/blog/hello`) ไม่ใช่ route pattern (`/blog/[slug]`) ครอบคลุมทุกกลยุทธ์
การ render: เอกสารใน cache จะถูกทิ้ง และสำหรับ SSG, ISR, PPR และ CSR คำขอถัดไปจะข้าม HTML ที่ build
เขียนลงดิสก์ด้วย — ไม่อย่างนั้นไฟล์นั้นจะถูกเสิร์ฟต่อไปเรื่อย ๆ และการ render ที่สำเร็จในคำขอถัดไป
จะเขียนทับไฟล์นั้นด้วย การ revalidate จึงจบลงจริง แทนที่จะต้องข้ามเอกสารเก่าใบเดิมไปตลอดอายุ โปรเซส
ส่วน build artifact ที่ไม่มีอยู่จะถูกปล่อยให้ไม่มีต่อไป ไม่ได้สร้างขึ้นใหม่ การ revalidate URL
ที่ยังไม่เคยมีใครร้องขอเป็นเรื่องปกติและเป็นเคสของ webhook ทั่วไป หนึ่ง request คิวได้สูงสุด 64 URL
ที่ไม่ซ้ำกัน และแต่ละ URL ยาวได้ไม่เกิน 2,048 ตัวอักษร `revalidatePath()` จะ throw
เมื่อเกินขอบเขตใดขอบเขตหนึ่ง ให้แบ่ง batch ที่ใหญ่กว่านี้เป็นหลาย request เพื่อไม่ให้ invalidation
รายการใดถูกทิ้งแบบเงียบ ๆ หาก render ล้มเหลว หรือเซิร์ฟเวอร์เขียน prerender directory ไม่ได้
ระบบจะคง revalidation ไว้เพื่อลองใหม่ใน request ถัดไป และเซิร์ฟเวอร์ที่เขียนไม่ได้เลยจะ log
คำเตือนเมื่อรายการที่ค้างเริ่มเต็ม

`revalidateTag()` เป็นฟังก์ชันคู่กันแต่ทำคนละหน้าที่:
[มันล้าง cache entry ที่ติด tag](#invalidate-ด้วย-tag-แทน-key) ส่วน `revalidatePath()` สั่ง render
เอกสารที่เก็บไว้ใหม่ tag ของ Ruvyxa กำกับค่าที่คุณ cache เองด้วย `cache().tags(...)`
ซึ่งไม่ใช่สิ่งเดียวกับ tag ของ Next.js — ที่นี่ไม่มี cache ของ `fetch()` ให้ tag ไปเกาะ และ tag
ไม่ได้ระบุ route การล้างข้อมูลที่หน้าหนึ่งอ่านจึงไม่ได้เขียน HTML ที่ build
สร้างไว้ใหม่ด้วยตัวมันเอง ถ้าต้องการให้เอกสารเปลี่ยน ให้เรียก `revalidatePath()`

บน deployment แบบ serverless `revalidatePath` จะล้าง function instance ที่เรียกมัน และคำขอถัดไป
จะเขียนเอกสารที่เก็บไว้ใหม่ให้กับคำขอหลังจากนั้นทั้งหมด ส่วน instance
อื่นที่อุ่นอยู่แล้วจะเสิร์ฟสำเนา ของตัวเองจนกว่า TTL ของมันจะหมด ซึ่งเป็นขอบเขตเดียวกับที่ ISR
มีอยู่แล้ว
