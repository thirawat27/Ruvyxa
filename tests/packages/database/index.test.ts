import assert from 'node:assert/strict'
import { describe, it } from 'node:test'

import {
  createDatabase,
  dynamoAdapter,
  prismaAdapter,
  requireDatabaseEnv,
  type DatabaseAdapter,
} from '../../../packages/@ruvyxa/database/dist/index.js'

interface TestSchema {
  users: { id: string; age: number; name: string }
}

describe('@ruvyxa/database', () => {
  it('forwards typed CRUD operations and validates destructive selectors', async () => {
    const calls: unknown[] = []
    const adapter: DatabaseAdapter = {
      name: 'capture',
      async execute(operation) {
        calls.push(operation)
        return operation.kind === 'findMany' ? [{ id: '1', age: 20, name: 'Ada' }] : { id: '1' }
      },
    }
    const db = createDatabase<TestSchema>(adapter)
    const users = await db.users.findMany({ where: { age: { gt: 18 } }, take: 50 })

    assert.equal(users[0]?.name, 'Ada')
    assert.deepEqual(calls[0], {
      model: 'users',
      kind: 'findMany',
      args: { where: { age: { gt: 18 } }, take: 50 },
    })
    await assert.rejects(() => db.users.update({ where: {}, data: { name: 'Grace' } }), /where/)
    await assert.rejects(() => db.users.findMany({ take: 10_001 }), /between 1 and 10000/)
  })

  it('uses adapter-owned transaction clients and fails when unsupported', async () => {
    const transactional: DatabaseAdapter = {
      name: 'transactional',
      async execute() {
        return 1
      },
      async transaction(run) {
        return run({ name: 'tx', execute: async () => 7 })
      },
    }
    const db = createDatabase<TestSchema>(transactional)
    assert.equal(await db.$transaction((tx) => tx.users.count()), 7)

    const withoutTransactions = createDatabase<TestSchema>({
      name: 'none',
      async execute() {
        return 0
      },
    })
    await assert.rejects(() => withoutTransactions.$transaction(async () => 1), /RUV3003/)
  })

  it('maps public models to Prisma-compatible delegates', async () => {
    const received: unknown[] = []
    const adapter = prismaAdapter(
      {
        user: {
          async findMany(args: unknown) {
            received.push(args)
            return [{ id: '1', name: 'Ada', age: 30 }]
          },
        },
      },
      { models: { users: 'user' } },
    )
    const db = createDatabase<TestSchema>(adapter)
    assert.equal((await db.users.findMany({ where: { age: { gte: 18 } } }))[0]?.id, '1')
    assert.deepEqual(received, [{ where: { age: { gte: 18 } } }])
  })

  it('requires explicit DynamoDB table mappings', async () => {
    const seen: unknown[] = []
    const adapter = dynamoAdapter({
      tables: { users: 'prod-users' },
      transport: {
        async execute(operation) {
          seen.push(operation)
          return []
        },
      },
    })
    const db = createDatabase<TestSchema>(adapter)
    await db.users.findMany()
    assert.equal((seen[0] as { table: string }).table, 'prod-users')

    const missing = createDatabase<{ posts: { id: string } }>(adapter)
    await assert.rejects(() => missing.posts.findMany(), /no configured table/)
  })

  it('rejects public secrets and names missing private environment at startup', () => {
    assert.throws(
      () => requireDatabaseEnv(['RUVYXA_PUBLIC_DATABASE_URL']),
      /refuses public database variable/,
    )
    assert.throws(() => requireDatabaseEnv(['not-a-name']), /not a valid variable name/)
    delete process.env.RUVYXA_TEST_DATABASE_URL
    assert.throws(
      () => requireDatabaseEnv(['RUVYXA_TEST_DATABASE_URL']),
      /RUV3001|missing private database environment variables: RUVYXA_TEST_DATABASE_URL/,
    )
    process.env.RUVYXA_TEST_DATABASE_URL = 'postgres://example'
    try {
      assert.doesNotThrow(() => requireDatabaseEnv(['RUVYXA_TEST_DATABASE_URL']))
    } finally {
      delete process.env.RUVYXA_TEST_DATABASE_URL
    }
  })
})
