#!/usr/bin/env node
/**
 * Call Demo — Simple Callable/Caller example
 *
 * Demonstrates:
 * 1. Callable server with handler
 * 2. Caller making RPC call
 *
 * Usage:
 *     node crates/jsbos/examples/call_demo.js
 */

const { Bus, version } = require('../jsbos.cjs');

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function main() {
  console.log(`\n  BrainOS JS — Call Demo (jsbos v${version()})`);
  console.log('📞'.repeat(30) + '\n');

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const callable = await bus.createCallable('rpc/add');
  callable.setHandler((err, input) => {
    const [a, b] = input.split(',').map(Number);
    return String(a + b);
  });
  await callable.start();
  console.log('  📡 Callable started: rpc/add');

  await sleep(200);

  const caller = await bus.createCaller('rpc/add');
  const result = await caller.callText('5,7');
  console.log(`  📤 Call: "5,7"`);
  console.log(`  📥 Response: "${result}"`);

  console.log('\n  ✅ Call demo done\n');
}

main().catch(console.error).finally(() => process.exit(0));
