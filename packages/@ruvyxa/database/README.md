# @ruvyxa/database

Typed, server-only database access for Ruvyxa. The package owns a small query contract and delegates
all network connections, pooling, migrations, and credentials to an explicit production adapter.

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

Register build-time secret validation in `ruvyxa.config.ts`:

```ts
import { config } from 'ruvyxa/config'
import { databasePlugin } from '@ruvyxa/database'

export default config({
  plugins: [databasePlugin({ requiredEnv: ['DATABASE_URL'] })],
})
```

The package deliberately does not export a process-global `db`: config plugins, middleware workers,
render workers, and serverless instances have different lifecycles. Create the client in a
server-only application module and let the selected driver own pooling for that process.
