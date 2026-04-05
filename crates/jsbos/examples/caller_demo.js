#!/usr/bin/env node
/**
 * Caller / Callable Demo — RPC pattern
 *
 * Demonstrates:
 * 1. Callable server with inline handler
 * 2. Callable with run() handler
 * 3. JSON request/response
 *
 * Usage:
 *     node crates/jsbos/examples/caller_demo.js
 */

const { Bus, version } = require('../jsbos.js');

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function demoInlineHandler() {
  console.log('═'.repeat(60));
  console.log('  Demo 1 — Callable with inline handler');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const callable = await bus.createCallable('rpc/add');
  await callable.start();
  console.log('  📡 Callable started: rpc/add');

  await sleep(200);

  const caller = await bus.createCaller('rpc/add');
  const result = await caller.callText('5,7');
  console.log(`  📤 Call: '5,7'`);
  console.log(`  📥 Response: '${result}'`);
  console.log('  ✅ Inline handler done\n');
}

async function demoRunHandler() {
  console.log('═'.repeat(60));
  console.log('  Demo 2 — Callable with run() handler');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const callable = await bus.createCallable('rpc/mul');
  callable.setHandler((err, input) => {
    const [a, b] = input.split(',').map(Number);
    return String(a * b);
  });
  await callable.start();
  console.log('  📡 Callable started: rpc/mul (custom handler: a*b)');

  await sleep(200);

  const caller = await bus.createCaller('rpc/mul');
  const result = await caller.callText('6,7');
  console.log(`  📤 Call: '6,7'`);
  console.log(`  📥 Response: '${result}'`);
  console.log('  ✅ Run handler done\n');
}

async function demoJsonRpc() {
  console.log('═'.repeat(60));
  console.log('  Demo 3 — Callable with JSON handler');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const callable = await bus.createCallable('rpc/greet');
  callable.setHandler((err, input) => {
    const data = JSON.parse(input);
    const name = data.name || 'World';
    return JSON.stringify({ greeting: `Hello, ${name}!`, status: 'ok' });
  });
  await callable.start();
  console.log('  📡 Callable started: rpc/greet (JSON handler)');

  await sleep(200);

  const caller = await bus.createCaller('rpc/greet');
  const result = await caller.callText(JSON.stringify({ name: 'Alice' }));
  const resp = JSON.parse(result);
  console.log(`  📤 Call: {"name": "Alice"}`);
  console.log(`  📥 Response: ${JSON.stringify(resp)}`);
  console.log('  ✅ JSON RPC done\n');
}

async function main() {
  console.log('\n' + '📞'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — Caller / Callable Demo`);
  console.log('📞'.repeat(30) + '\n');

  await demoInlineHandler();
  await demoRunHandler();
  await demoJsonRpc();

  console.log('═'.repeat(60));
  console.log('  ✅ All Caller/Callable demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));
