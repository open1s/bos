#!/usr/bin/env node
/**
 * Full Integration Demo — All brainos components working together
 *
 * Demonstrates:
 * 1. Config loading and discovery
 * 2. Bus lifecycle
 * 3. Pub/Sub event streaming
 * 4. Query/Queryable request-response
 * 5. Caller/Callable RPC
 *
 * Usage:
 *     node crates/jsbos/examples/integration_demo.js
 */

const { Bus, ConfigLoader, version } = require('../index.js');

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function demoConfigAndBus() {
  console.log('═'.repeat(60));
  console.log('  Step 1 — Config + Bus lifecycle');
  console.log('═'.repeat(60));

  const cfg = new ConfigLoader();
  cfg.discover();
  const data = JSON.parse(cfg.loadSync());
  console.log(`  ⚙️  Config keys: ${Object.keys(data).length > 0 ? Object.keys(data).join(', ') : '(empty)'}`);

  const bus = await Bus.create();
  console.log('  🚌 Bus started (mode=peer)');

  const sub = await bus.createSubscriber('system/ready');
  await bus.publishText('system/ready', 'all components online');
  console.log("  📨 Published: system/ready = 'all components online'");

  await sleep(200);
  const msg = await sub.recvWithTimeoutMs(1000);
  console.log(`  📨 Received: '${msg}'`);

  console.log('  ✅ Config + Bus done\n');
}

async function demoPubSub() {
  console.log('═'.repeat(60));
  console.log('  Step 2 — Pub/Sub event streaming');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  const pub = await bus.createPublisher('events/user-action');
  const sub = await bus.createSubscriber('events/user-action');

  const recvPromise = sub.recvWithTimeoutMs(2000);
  await sleep(100);

  await pub.publishJson({
    action: 'login',
    user: 'alice',
    timestamp: '2026-04-03T10:00:00Z',
  });

  const result = await recvPromise;
  if (result) {
    // Result is already parsed as JSON by publishJson, but recv returns string
    // So we need to parse it
    const parsed = JSON.parse(result);
    console.log(`  📨 Event received: user=${parsed.user}, action=${parsed.action}`);
  }

  console.log('  ✅ Pub/Sub done\n');
}

async function demoQuery() {
  console.log('═'.repeat(60));
  console.log('  Step 3 — Query/Queryable request-response');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  const queryable = await bus.createQueryable('svc/wordcount');
  await queryable.start();

  await sleep(200);

  const query = await bus.createQuery('svc/wordcount');
  const resp = await query.queryText('hello world from brainos');
  console.log(`  📤 Query: 'hello world from brainos'`);
  console.log(`  📥 Response: '${resp}'`);

  console.log('  ✅ Query done\n');
}

async function demoRpc() {
  console.log('═'.repeat(60));
  console.log('  Step 4 — Caller/Callable RPC');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  const callable = await bus.createCallable('rpc/reverse');
  callable.setHandler((err, input) => input.split('').reverse().join(''));
  await callable.start();

  await sleep(200);

  const caller = await bus.createCaller('rpc/reverse');
  const result = await caller.callText('brainos');
  console.log(`  📤 Call: 'brainos'`);
  console.log(`  📥 Response: '${result}'`);

  console.log('  ✅ RPC done\n');
}

async function main() {
  console.log('\n' + '🧠'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — Full Integration Demo`);
  console.log('🧠'.repeat(30) + '\n');

  await demoConfigAndBus();
  await demoPubSub();
  await demoQuery();
  await demoRpc();

  console.log('═'.repeat(60));
  console.log('  ✅ All integration demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));
