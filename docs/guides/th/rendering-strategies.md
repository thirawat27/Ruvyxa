# Rendering Strategies

ลำดับการตรวจสอบ (rule แรกที่ match จะถูกใช้):

| ลำดับ | Declaration                          | Strategy |
| ----- | ------------------------------------ | -------- |
| 1     | `'use client'`                       | CSR      |
| 2     | `export const ppr = true`            | PPR      |
| 3     | `export const revalidate = 60`       | ISR      |
| 4     | `export const getStaticParams = ...` | SSG      |
| 5     | Static route (no dynamic markers)    | SSG      |
| 6     | Default                              | SSR      |

## SSR (Default)

```tsx
export default async function ProductPage() {
  const products = await db.products.findMany()
  return <ProductList items={products} />
}
```

## SSG + getStaticParams

```tsx
export const getStaticParams: GetStaticParams<{ slug: string }> = async () => [
  { slug: 'getting-started' },
  { slug: 'deployment' },
]
```

## ISR

```tsx
export const revalidate = 60
```

## PPR

```tsx
export const ppr = true
```

ตรวจสอบ strategy: `npx ruvyxa routes`
