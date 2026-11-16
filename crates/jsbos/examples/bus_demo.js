#!/usr/bin/env node
/**
 * Bus Demo — Pub/Sub event streaming
 *
 * Demonstrates:
 * 1. Bus creation
 * 2. Direct publish_text via Bus
 * 3. Publisher / Subscriber creation via Bus factory methods
 * 4. Subscriber with timeout
 *
 * Usage:
 *     node crates/jsbos/examples/bus_demo.js
 */

import { Bus, version } from '../index.js'

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

async function demoDirectPublish() {
  console.log('═'.repeat(60))
  console.log('  Demo 1 — Direct publish via Bus')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  console.log('  🚌 Bus created')

  const sub = await bus.createSubscriber('demo/greet')
  const recvPromise = sub.recvWithTimeoutMs(3000)

  await sleep(300)

  await bus.publishText('demo/greet', 'Hello from jsbos!')
  console.log('  📨 Published: "Hello from jsbos!"')

  const msg = await recvPromise
  if (msg) {
    console.log(`  📨 Received: "${msg}"`)
  } else {
    console.log('  ℹ️  Message not received (timing issue)')
  }

  console.log('  ✅ Direct publish done\n')
}

async function demoPublisherSubscriber() {
  console.log('═'.repeat(60))
  console.log('  Demo 2 — Publisher & Subscriber objects')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  console.log('  🚌 Bus created')

  const pub = await bus.createPublisher('demo/events')
  const sub = await bus.createSubscriber('demo/events')
  console.log(`  📤 Publisher on: ${pub.topic}`)
  console.log(`  📨 Subscriber on: ${sub.topic}`)

  const recvPromise = sub.recvWithTimeoutMs(3000)
  await pub.publishText('event-fired')
  console.log('  📨 Published: "event-fired"')

  const msg = await recvPromise
  if (msg) {
    console.log(`  📨 Subscriber received: "${msg}"`)
  } else {
    console.log('  ℹ️  Message not received (timing issue)')
  }

  console.log('  ✅ Publisher/Subscriber done\n')
}

async function demoBatchPublish() {
  console.log('═'.repeat(60))
  console.log('  Demo 3 — Batch publish')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  const pub = await bus.createPublisher('demo/stream')
  const sub = await bus.createSubscriber('demo/stream')

  for (let i = 0; i < 3; i++) {
    await pub.publishText(`message-${i}`)
    console.log(`  📤 Published: message-${i}`)
    await sleep(50)
  }

  for (let i = 0; i < 3; i++) {
    const msg = await sub.recvWithTimeoutMs(2000)
    if (msg) {
      console.log(`  📨 Received: "${msg}"`)
    }
  }

  console.log('  ✅ Batch publish done\n')
}

async function main() {
  console.log('\n' + '🚌'.repeat(30))
  console.log(`  BrainOS (jsbos v${version()}) — Bus Demo`)
  console.log('🚌'.repeat(30) + '\n')

  await demoDirectPublish()
  await demoPublisherSubscriber()
  await demoBatchPublish()

  console.log('═'.repeat(60))
  console.log('  ✅ All Bus demos completed!')
  console.log('═'.repeat(60) + '\n')
}

main().catch(console.error)