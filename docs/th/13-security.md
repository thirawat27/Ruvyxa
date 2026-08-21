# Security

> **เป้าหมายของ tutorial:** เปลี่ยน safeguard ของ framework ให้เป็น security routine ของแอป
> **เริ่มจาก:** configuration ของคุณใน [Configuration](07-configuration.md) **Checkpoint:** ตรวจ
> application checklist และพิสูจน์ protected boundary หนึ่งจุดในแอปของคุณ

security เป็นหลายชั้น: framework validation และ default ลดความเสี่ยง แต่ application authorization,
secret storage, upstream network control และ infrastructure policy ยังเป็นความรับผิดชอบของคุณ

## Control ที่ framework บังคับ

- boundary validation ปฏิเสธ private environment access และ server-only import ใน client code;
  browser-safe value ต้องมี prefix `RUVYXA_PUBLIC_`
- จำกัดขนาด body ของ action/API ได้ใน `security` action rate limit ตั้ง maximum/window ได้; action
  input schema ทำงานก่อน action handler
- `security.trustedProxyIps` เป็น allow-list สำหรับ forwarded IP/protocol header; loopback proxy
  ได้รับความเชื่อถือโดยปริยาย อย่าเชื่อ forwarded header จาก client ทั่วไป
- `middleware.builtin.rate` จำกัดอัตราให้ทุก route ไม่ใช่เฉพาะ action key ปริยาย `ip` คือ transport
  peer เว้นแต่ peer นั้นเป็น loopback หรืออยู่ใน `security.trustedProxyIps` กรณีนั้นจะไล่ chain ของ
  forwarded header จากขวาหา address แรกที่ไม่ใช่ proxy ของคุณ — client ที่ไม่ใช่ proxy
  จึงเปลี่ยนชื่อ ตัวเองไม่ได้ เมื่ออยู่หลัง reverse proxy ให้ใช้ `ip` นี่แหละ: ระบุ proxy ใน
  `trustedProxyIps` แล้ว ปล่อย key ไว้ตามเดิม ส่วน `header:<name>` เป็นทางออกสำหรับ identity ที่
  application กำหนดเอง เช่น API key และถูกใช้ตามค่าที่ส่งมาตรง ๆ การชี้ไปที่ `x-forwarded-for`
  จึงเท่ากับยกกุญแจ bucket ให้ผู้เรียก และ client เดียวหมุนค่าเพื่อขอโควตาไม่จำกัดได้
  เซิร์ฟเวอร์จะเตือนตอน startup เมื่อพบการตั้งค่าแบบนั้น
- first-party `redirects` plugin validate destination ต่อ scheme-relative, backslash และ
  invalid-origin form ที่ไม่ปลอดภัย `securityHeaders` validate CSP directive map และให้ HSTS เป็น
  default
- auth code มี signed/session/provider runtime และ rate-limit store contract แต่ durable storage และ
  cookie/origin decision ที่ขึ้นกับ deployment เป็นงานของ application

## Application checklist

- validate ทุก API body, route parameter และ action input size limit ไม่ใช่ semantic validation
- authorize ทุก data read/write ใน handler; `ActionContext.user` เป็น optional และไม่ได้
  authenticate caller เอง
- เก็บ `RUVYXA_AUTH_SECRET`, OAuth secret และ database credential นอก source control ห้ามใช้
  `RUVYXA_PUBLIC_` สำหรับสิ่งเหล่านี้
- ระบุ CORS origin/method/header ให้ชัดเมื่อใช้ `middleware.builtin.cors`; อย่าเปิด credentialed
  cross-origin access โดยไม่มี origin list ที่ review ทั้งสามค่าไม่มี default — ถ้าไม่ระบุ `methods`
  หรือ `headers` จะไม่ส่ง `Access-Control-Allow-Methods` หรือ `Access-Control-Allow-Headers` เลย
  ดังนั้น cross-origin request ที่ใช้อะไรเกิน simple method จะถูกบล็อกจนกว่าจะระบุเอง และ
  credentials คู่กับ `origins: ['*']` จะถูกปฏิเสธทันที
- ใช้ route-scoped CSP, frame, referrer, COOP/COEP/CORP และ permissions policy ผ่าน
  `securityHeaders` หลังตรวจ asset ที่ต้องใช้
- อย่าให้ structured log มี token, cookie, authorization header, request body หรือข้อมูลส่วนบุคคล
  observability plugin log method/path/status/timing ไม่ใช่ redaction solution ทั่วไป

## Infrastructure checklist

terminate TLS, จำกัด inbound network, ตั้ง process memory/time limit, patch Node/Rust/dependency
และให้ secret manager ใส่เฉพาะ proxy address/CIDR ที่รู้จักใน `trustedProxyIps` ทดสอบ authentication
redirect ด้วย production origin การป้องกัน cross-site สำหรับ route handler คือ plugin `originGuard`
ซึ่งเปิดใช้เองตาม route scope ส่วน rate limiting ทั่วไปคือ `middleware.builtin.rate`
ซึ่งปิดอยู่จนกว่าจะ ตั้งค่า ไม่พบหลักฐานว่ามี malware scanning, WAF หรือ automatic
dependency-vulnerability remediation; เพิ่ม control เหล่านี้เมื่อ threat model ต้องการ

**ก่อนหน้า:** [Development และ testing](12-development-testing.md) · **ถัดไป:**
[Observability และ performance](14-observability-performance.md)
