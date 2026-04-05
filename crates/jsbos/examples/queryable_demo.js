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

const { Bus, Query, Queryable, version } = require('../jsbos.js');

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function testQueryableRunTextHandler() {
  console.log('═'.repeat(60));
  console.log('  Test — Queryable.run() with text handler');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  // Handler that echoes back the request
  const handler = (err, request) => {
    const data = JSON.parse(request);
    data.echo = true;
    return JSON.stringify(data);
  };

  // Create queryable with run handler
  const queryable = await bus.createQueryable('test/echo');
  console.log('  📡 Queryable created: test/echo');

  // Start handler in background
  const runPromise = queryable.run(handler);
  
  // Give handler time to start and register with Zenoh
  await sleep(200);

  // Make a call from another client
  const query = await bus.createQuery('test/echo');
  console.log(`  🔍 Query created: ${query.topic}`);
  
  const response = await query.queryText(JSON.stringify({ msg: 'hello' }));
  console.log(`  📥 Response: ${response}`);
  
  // Verify response
  const result = JSON.parse(response);
  if (result.msg === 'hello' && result.echo === true) {
    console.log('  ✅ Queryable.run() text handler test passed');
  } else {
    console.log('  ❌ Test failed: unexpected response');
  }

  console.log('');
}

async function main() {
  console.log('\n' + '📝'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — Queryable Demo`);
  console.log('📝'.repeat(30) + '\n');

  await testQueryableRunTextHandler();

  console.log('═'.repeat(60));
  console.log('  ✅ Queryable demo completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));