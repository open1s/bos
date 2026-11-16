#!/usr/bin/env node
/**
 * Logging Demo — Initialize tracing from JavaScript
 *
 * Demonstrates:
 * 1. Initialize logging via initTracing()
 * 2. Log level comes from BOS_LOG env or config file
 *
 * Usage:
 *     BOS_LOG=debug node crates/jsbos/examples/logging_demo.js
 *     # or
 *     node crates/jsbos/examples/logging_demo.js
 *
 * Logs are written to ~/.bos/log/
 */

import { initTracing, version, Bus } from '../index.js'

async function main() {
  console.log('\n' + '📝'.repeat(30))
  console.log(`  BrainOS (jsbos v${version()}) — Logging Demo`)
  console.log('📝'.repeat(30) + '\n')

  console.log('  🔧 Initializing tracing via initTracing()...')
  initTracing()
  console.log('  ✅ Tracing initialized')

  console.log('\n  Creating bus...')
  const bus = await Bus.create()
  console.log('  🚌 Bus created')

  const pub = await bus.createPublisher('demo/logs')
  await pub.publishText('Hello with logging!')
  console.log('  📤 Published message')

  const queryable = await bus.createQueryable('demo/echo', (msg) => msg.toUpperCase())
  await queryable.start()
  console.log('  🔧 Queryable started')

  const query = await bus.createQuery('demo/echo')
  const response = await query.queryText('hello query')
  console.log(`  👀 Query response: "${response}"`)

  console.log('\n  Logs written to ~/.bos/log/')
  console.log('═'.repeat(60))
  console.log('  ✅ Logging demo completed!')
  console.log('═'.repeat(60) + '\n')
}

main().catch(console.error)