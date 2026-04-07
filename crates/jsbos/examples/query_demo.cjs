#!/usr/bin/env node
/**
 * Query / Queryable Demo — Request/Response pattern
 *
 * Demonstrates:
 * 1. Queryable server with inline handler
 * 2. Queryable with run() handler
 * 3. Query with timeout
 *
 * Usage:
 *     node crates/jsbos/examples/query_demo.js
 */

const { Bus, version } = require('../index.js');

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function demoInlineHandler() {
  console.log('═'.repeat(60));
  console.log('  Demo 1 — Queryable with inline handler');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const queryable = await bus.createQueryable('svc/upper');
  await queryable.start();
  console.log('  📡 Queryable started on: svc/upper');

  await sleep(200);

  const query = await bus.createQuery('svc/upper');
  const result = await query.queryText('hello world');
  console.log(`  📤 Query: 'hello world'`);
  console.log(`  📥 Response: '${result}'`);
  console.log('  ✅ Inline handler done\n');
}

async function demoRunHandler() {
  console.log('═'.repeat(60));
  console.log('  Demo 2 — Queryable with run() handler');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const queryable = await bus.createQueryable('svc/echo');
  await queryable.start();
  console.log('  📡 Queryable started on: svc/echo');

  await sleep(200);

  const query = await bus.createQuery('svc/echo');
  const payload = JSON.stringify({ msg: 'ping' });
  const resp = await query.queryText(payload);
  const result = JSON.parse(resp);
  console.log(`  📤 Query: ${JSON.stringify(result)}`);
  console.log('  ✅ Run handler done\n');
}

async function demoTimeout() {
  console.log('═'.repeat(60));
  console.log('  Demo 3 — Query with timeout');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const queryable = await bus.createQueryable('svc/slow');
  await queryable.start();
  console.log('  📡 Queryable started on: svc/slow');

  await sleep(200);

  const query = await bus.createQuery('svc/slow');
  const result = await query.queryTextTimeoutMs('test-data', 5000);
  console.log(`  📤 Query with 5s timeout: 'test-data'`);
  console.log(`  📥 Response: '${result}'`);
  console.log('  ✅ Timeout query done\n');
}

async function main() {
  console.log('\n' + '🔍'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — Query / Queryable Demo`);
  console.log('🔍'.repeat(30) + '\n');

  await demoInlineHandler();
  await demoRunHandler();
  await demoTimeout();

  console.log('═'.repeat(60));
  console.log('  ✅ All Query demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error);
