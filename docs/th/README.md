# เอกสาร Ruvyxa

## เรียนรู้ Ruvyxa แบบ tutorial

เริ่มที่บท 1 และให้แอปขนาดเล็กหนึ่งตัวรันอยู่ตลอดขณะเรียนบท 2–7 ทุกบทจะบอกเป้าหมาย
บทก่อนหน้าที่ควรอ่าน และ checkpoint ที่ต้องทำให้ครบก่อนกดลิงก์ **ถัดไป** บท 8–21 เป็น tutorial
เฉพาะเรื่องและ reference ที่กลับมาใช้ได้เมื่อแอปของคุณเติบโตขึ้น

Ruvyxa คือ Web framework ที่ให้ CLI, pipeline สำหรับ route/build ที่เขียนด้วย Rust และ runtime
TypeScript ทำงานร่วมกันเพื่อค้นหา route ใน `app/`, compile และ serve หรือจัดแพ็กเกจผลลัพธ์
คู่มือนี้อธิบายพฤติกรรมที่มี implementation อยู่จริงใน repository ณ revision ที่คุณกำลังอ่านอยู่

## เลือกเส้นทางตามบทบาท

- ผู้พัฒนาแอปที่เริ่มต้นใหม่: [บทนำ](01-introduction.md) →
  [สร้าง app แรก](02-create-your-first-app.md) → [โครงสร้างโปรเจกต์](03-project-structure.md) →
  [Routing และ rendering](04-routing-rendering.md)
- ผู้พัฒนา full-stack: อ่านต่อ [ข้อมูล, action และ API route](05-data-actions-api.md),
  [UI, navigation, metadata และ asset](06-ui-navigation-metadata-and-assets.md), และ
  [Configuration](07-configuration.md)
- ผู้พัฒนา integration: อ่าน [Request pipeline](08-request-pipeline.md),
  [การเชื่อมต่อ](09-integrations-auth-data-and-realtime.md), และ
  [Public API reference](17-public-api-reference.md)
- ผู้ดูแลระบบ: เริ่ม [CLI](10-cli.md), แล้วอ่าน [Security](13-security.md),
  [Observability และ performance](14-observability-performance.md), และ
  [Deploy, run และ operate](15-deploy-run-and-operate.md)
- ผู้พัฒนา framework: ใช้ [Architecture](11-architecture.md) และ
  [Development และ testing](12-development-testing.md)

## บททั้งหมด

1. [บทนำ](01-introduction.md)
2. [สร้าง Ruvyxa app แรก](02-create-your-first-app.md)
3. [โครงสร้างโปรเจกต์](03-project-structure.md)
4. [Routing และ rendering](04-routing-rendering.md)
5. [ข้อมูล, action และ API route](05-data-actions-api.md)
6. [UI, navigation, metadata และ asset](06-ui-navigation-metadata-and-assets.md)
7. [Configuration และ environment](07-configuration.md)
8. [Request pipeline](08-request-pipeline.md)
9. [การเชื่อมต่อ: authentication, data, realtime, adapter และ testing](09-integrations-auth-data-and-realtime.md)
10. [CLI reference](10-cli.md)
11. [Architecture](11-architecture.md)
12. [Development และ testing](12-development-testing.md)
13. [Security](13-security.md)
14. [Observability และ performance](14-observability-performance.md)
15. [Deploy, run และ operate ใน production](15-deploy-run-and-operate.md)
16. [Troubleshooting และ compatibility เมื่ออัปเกรด](16-troubleshooting-upgrades.md)
17. [Public API reference](17-public-api-reference.md)
18. [ขอบเขตเอกสารและแหล่งข้อมูล](18-documentation-scope-and-sources.md)
19. [Release-readiness playbook](19-release-readiness-playbook.md)
20. [คู่มือ platform adapter](20-platform-adapter-guide.md)
21. [Practical recipes](21-practical-recipes.md)
