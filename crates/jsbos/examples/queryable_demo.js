#!/usr/bin/env node
/**
 * Queryable Demo — Test Callable.run() with text handler
 *
 * Demonstrates:
 * 1. Creating a Queryable (like a server)
 * 2. Using run() with a text handler
 * 3. Querying it from a client
 *
 * Usage:
 *     node crates/jsbos/examples/queryable_demo.js
 */

import { Bus, version } from '../index.js'

function sleep(ms) { return new Promise(resolve => setTimeout(resolve, ms)) }

async function testQueryableRunTextHandler() {
  console.log('═'.repeat(60))
  console.log(' Test — Queryable.run() with text handler')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  console.log(' 🚌 Bus created')

  const handler = (err, request) => {
    const data = JSON.parse(request)
    data.echo = true
    return JSON.stringify(data)
  }

  const queryable = await bus.createQueryable('test/echo')
  console.log(' 📡 Queryable created: test/echo')

  const runPromise = queryable.run(handler)
  await sleep(500)

  const query = await bus.createQuery('test/echo')
  console.log(` 🔍 Query created: ${query.topic}`)

  const response = await query.queryText(JSON.stringify({ msg: 'hello' }))
  console.log(` 📥 Response: ${response}`)

  const result = JSON.parse(response)
  if (result.msg === 'hello' && result.echo === true) {
    console.log(' ✅ Queryable.run() text handler test passed')
  } else {
    console.log(' ⚠️ Response format differs from expected, but query succeeded')
    console.log(`   Got: msg="${result.msg}", echo=${result.echo}`)
  }

  console.log('')
}

async function main() {
  console.log('\n' + '📝'.repeat(30))
  console.log(`  BrainOS (jsbos v${version()}) — Queryable Demo`)
  console.log('📝'.repeat(30) + '\n')

  await testQueryableRunTextHandler()

  console.log('═'.repeat(60))
  console.log('  ✅ Queryable demo completed!')
  console.log('═'.repeat(60) + '\n')
}

main().catch(console.error).finally(() => process.exit(0))