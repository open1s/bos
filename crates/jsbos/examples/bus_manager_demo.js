#!/usr/bin/env node
/**
 * BusManager Demo — Pub/Sub with async context manager
 *
 * Demonstrates:
 * 1. BusManager lifecycle with async context manager
 * 2. Direct publish_text / publish_json
 * 3. Publisher / Subscriber creation and usage
 * 4. Subscriber as async iterator
 * 5. Subscriber with callback loop
 */

import { Bus } from '../index.js'

async function demoDirectPublish() {
  console.log('═'.repeat(60))
  console.log('  Demo 1 — Direct publish via Bus')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  const sub = await bus.createSubscriber('demo/greet')

  const recvPromise = (async () => {
    const msg = await sub.recvWithTimeoutMs(2000)
    if (msg) {
      console.log(`  📨 Received: ${msg}`)
    }
  })()

  await new Promise(r => setTimeout(r, 100))

  await bus.publishText('demo/greet', 'Hello from Bus!')
  await recvPromise

  console.log('  ✅ Direct publish done\n')
}

async function demoPublisherSubscriber() {
  console.log('═'.repeat(60))
  console.log('  Demo 2 — Publisher & Subscriber objects')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  const pub = await bus.createPublisher('demo/events')
  const sub = await bus.createSubscriber('demo/events')

  const recvPromise = (async () => {
    const msg = await sub.recvWithTimeoutMs(2000)
    if (msg) {
      console.log(`  📨 Subscriber received: ${msg}`)
    }
  })()

  await new Promise(r => setTimeout(r, 100))

  await pub.publishText('event-fired')
  await recvPromise

  await pub.publishJson({ action: 'deploy', service: 'api', version: '2.0' })
  const jsonMsg = await sub.recvJsonWithTimeoutMs(2000)
  if (jsonMsg) {
    console.log(`  📨 JSON received: action=${jsonMsg.action}, service=${jsonMsg.service}, version=${jsonMsg.version}`)
  }

  console.log('  ✅ Publisher/Subscriber done\n')
}

async function demoAsyncIterator() {
  console.log('═'.repeat(60))
  console.log('  Demo 3 — Subscriber as async iterator (manual)')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  const pub = await bus.createPublisher('demo/stream')
  const sub = await bus.createSubscriber('demo/stream')

  const publishBatch = async () => {
    for (let i = 0; i < 3; i++) {
      await pub.publishText(`message-${i}`)
      await new Promise(r => setTimeout(r, 50))
    }
  }

  const pubTask = publishBatch()

  let count = 0
  while (count < 3) {
    const msg = await sub.recvWithTimeoutMs(1000)
    if (msg) {
      console.log(`  📨 Received: ${msg}`)
      count++
    }
  }

  await pubTask
  console.log('  ✅ Async iterator done\n')
}

async function demoCallbackLoop() {
  console.log('═'.repeat(60))
  console.log('  Demo 4 — Subscriber callback loop')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  const pub = await bus.createPublisher('demo/callback')
  const sub = await bus.createSubscriber('demo/callback')

  const received = []

  await sub.run((msg) => {
    const text = (msg !== null && msg !== undefined) ? msg : 'callback-msg'
    received.push(text)
    console.log(`  📨 Callback received: ${text}`)
    return text
  })

  await new Promise(r => setTimeout(r, 100))

  for (let i = 0; i < 3; i++) {
    await pub.publishText(`callback-msg-${i}`)
    await new Promise(r => setTimeout(r, 50))
  }

  await new Promise(r => setTimeout(r, 200))
  await sub.stop()
  console.log(`  ✅ Callback loop done — received ${received.length} messages\n`)
}

async function main() {
  console.log('\n' + '🚌'.repeat(30))
  console.log('  BrainOS — BusManager Demo')
  console.log('🚌'.repeat(30) + '\n')

  await demoDirectPublish()
  await demoPublisherSubscriber()
  await demoAsyncIterator()
  await demoCallbackLoop()

  console.log('═'.repeat(60))
  console.log('  ✅ All BusManager demos completed!')
  console.log('═'.repeat(60) + '\n')
}

main()