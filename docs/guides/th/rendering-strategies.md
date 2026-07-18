# Rendering Strategies

ลำดับการตรวจสอบ (rule แรกที่ match จะถูกใช้):

| ลำดับ | Declaration                           | Strategy |
| ----- | ------------------------------------- | -------- |
| 1     | `'use client'`                        | CSR      |
| 2     | `export const ppr = true`             | PPR      |
| 3     | `export const revalidate = 60`        | ISR      |
| 4     | `getStaticParams` หรือ `staticParams` | SSG      |
| 5     | Static route (no dynamic markers)     | SSG      |
| 6     | Default                               | SSR      |

## SSR (Default)

```tsx
export default async function ProductPage() {
  const products = await db.products.findMany()
  return <ProductList items={products} />
}
```

## SSG parameters แบบตรงไปตรงมา

ถ้ารู้ค่าล่วงหน้าและ route มี dynamic segment เดียว ให้ export เป็น scalar array ได้ทันที:

```tsx
// app/articles/[slug]/page.tsx
export const staticParams = ['getting-started', 'deployment']
```

ถ้ามีหลาย dynamic segments ให้ใช้ object:

```tsx
export const staticParams = [
  { category: 'guides', slug: 'getting-started' },
  { category: 'news', slug: 'release-1-0-15' },
]
```

## SSG + getStaticParams

```tsx
export const getStaticParams: GetStaticParams<{ slug: string }> = async ({ route, routes }) => {
  console.log(`Generating ${route.path}; พบทั้งหมด ${routes.length} routes`)
  return ['getting-started', 'deployment']
}
```

context ประกอบด้วย route ปัจจุบัน, ข้อมูล dynamic segments และ routes ทั้งหมดที่ค้นพบ สำหรับ
catch-all route ค่า scalar จะถูกแปลงเป็น string array ที่มีหนึ่งสมาชิก

หากการค้นหา params มีต้นทุนสูง สามารถเปิด persistent cache แบบ TTL ได้:

```tsx
export const getStaticParams: GetStaticParams<{ slug: string }> = async () => ({
  params: (await fetchPosts()).map((post) => post.slug),
  cache: '10m',
})
```

`cache` รับจำนวนวินาทีหรือ duration ที่ลงท้ายด้วย `s`, `m`, `h`, `d` ตั้งแต่ 1 วินาทีถึง 365 วัน
cache จะหมดอายุเอง และ invalidate ก่อนเวลาเมื่อ page, imported dependency, route metadata หรือ route
manifest เปลี่ยน หาก return array โดยตรงจะยังคงไม่ cache เช่นเดิม

## ISR

```tsx
export const revalidate = 60
```

ระบบจะส่ง cached output เดิมระหว่าง regenerate และจะเริ่มงาน background หลังครบช่วงเวลาที่กำหนด โดย
request พร้อมกันของ route เดียวกันจะใช้การ refresh เดียวร่วมกัน

## PPR

```tsx
export const ppr = true
```

ตรวจสอบ strategy: `npx ruvyxa routes`
