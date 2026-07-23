# Styling, SCSS และ CSS Modules

> 🟢 **เหมาะกับมือใหม่** · ⏱️ อ่าน ~5 นาที
>
> **จะได้เรียนรู้:** global stylesheet, Sass/SCSS และ CSS Modules รายคอมโพเนนต์ — hot reload ครบ
> ไม่ต้องตั้งค่าอะไรเพิ่ม

Ruvyxa รองรับ global CSS, SCSS/Sass และ CSS Modules แบบ local scope ผ่าน module graph ตามปกติ
ไฟล์ที่ import สามารถอยู่ที่ใดก็ได้ภายในโปรเจค

## Global CSS และ SCSS

Import stylesheet จาก layout หรือ component ได้โดยตรง:

```tsx
import './globals.scss'
```

ระบบ compile ทั้ง `.scss` และ `.sass` ให้อัตโนมัติ รวมถึงติดตาม partials ที่อ้างด้วย `@use`,
`@forward` หรือ `@import` เพื่อให้ HMR ทำงานเมื่อไฟล์ที่เกี่ยวข้องเปลี่ยน

สำหรับ global stylesheet ที่ไม่ได้ import ให้กำหนดผ่าน `css.entries`:

```ts
import { config } from 'ruvyxa/config'

export default config({
  css: { entries: ['styles/theme.scss'] },
})
```

## CSS Modules

ตั้งชื่อไฟล์เป็น `.module.css`, `.module.scss` หรือ `.module.sass` แล้ว import ค่า default:

```scss
// app/card.module.scss
$accent: #7c3aed;

.card {
  border: 1px solid $accent;

  .title {
    color: $accent;
  }
}
```

```tsx
import styles from './card.module.scss'

export function Card() {
  return <article className={styles.card}>Scoped card</article>
}
```

ค่า default คือ class map โดยชื่อใหม่สร้างจาก project-relative path และชื่อ class เดิมแบบ
deterministic CSS ที่ส่งออกใช้ชื่อเดียวกัน จึงไม่ชนกันข้าม component และผล build ทำซ้ำได้ ทั้ง
production minification และ dev HMR ใช้ mapping เดียวกัน

CSS Modules จะ scope local class selectors รวมถึง nested CSS ด้วย `:global(.name)` จะคง selector
นั้นเป็น global ส่วน `composes: other;` แบบ local จะเพิ่ม class ที่ compose ลงใน class map และเอา
declaration ออกจาก CSS ที่ส่งออก การ compose ข้ามไฟล์ (`composes: other from './other.module.css'`)
ยังไม่รองรับใน built-in transformer ให้ compose class ใน code แทน

TypeScript declarations มาจาก package `ruvyxa` โดยตรง จึงไม่ต้องสร้าง `css.d.ts` ในแอป ส่วน LESS
ยังไม่อยู่ใน built-in pipeline และจะแสดง diagnostic หาก import โดยไม่มี transform plugin
