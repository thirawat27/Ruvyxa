<p align="center">
  <a href="https://github.com/thirawat27/Ruvyxa">
    <img src="https://raw.githubusercontent.com/thirawat27/Ruvyxa/main/assets/branding/ruvyxa.png" alt="Ruvyxa" width="140" height="140" />
  </a>
</p>

<h1 align="center">@ruvyxa/database</h1>

<p align="center">
  Typed, server-only database access for Ruvyxa. The package owns a small query contract and<br/>
  delegates all network connections, pooling, migrations, and credentials to an explicit<br/>
  production adapter.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@ruvyxa/database"><img src="https://img.shields.io/npm/v/@ruvyxa/database?style=flat-square" alt="npm version" /></a>
  <a href="https://www.npmjs.com/package/@ruvyxa/database"><img src="https://img.shields.io/node/v/@ruvyxa/database?style=flat-square&label=node" alt="Supported Node version" /></a>
  <img src="https://img.shields.io/badge/license-Apache%202.0-green?style=flat-square" alt="License" />
</p>

---

```ts
import { PrismaClient } from '@prisma/client'
import { createDatabase, prismaAdapter } from '@ruvyxa/database'

interface Schema {
  users: { id: string; email: string; age: number }
}

const prisma = new PrismaClient()
export const db = createDatabase<Schema>(prismaAdapter(prisma, { models: { users: 'user' } }))

const adults = await db.users.findMany({ where: { age: { gt: 18 } } })
```

Prisma-compatible delegates cover PostgreSQL, MySQL, SQLite, and MongoDB. `dynamoAdapter()` accepts
an explicit transport so AWS SDK v2/v3 or a DynamoDB-compatible service can execute the same
normalized operations without this package pinning an AWS SDK version. Custom drivers implement
`DatabaseAdapter` or use `defineDatabaseAdapter()`.

Refuse to start without the private environment the database needs, where the process begins:

```ts
// instrumentation.ts
import { requireDatabaseEnv } from '@ruvyxa/database'

export function register() {
  requireDatabaseEnv(['DATABASE_URL'])
}
```

`requireDatabaseEnv()` throws `RUV3001` naming every missing variable and refuses a `RUVYXA_PUBLIC_`
name outright, since that prefix ships to browsers.

The package deliberately does not export a process-global `db`: render workers and serverless
instances have different lifecycles. Create the client in a server-only application module and let
the selected driver own pooling for that process.
