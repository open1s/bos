// Behavioral coverage for the bus, which the JS suite previously did not
// exercise at all: publish/subscribe round-trips through the native Bus and
// through the Publisher and Subscriber objects it hands out.
import test from 'ava'
import { Bus } from '../index.js'

let seq = 0
function topic(name) {
  seq += 1
  return 'jsbos/test/' + name + '/' + process.pid + '/' + seq
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

async function newBus(t) {
  const bus = await Bus.create()
  t.teardown(async () => {
    try {
      await bus.close()
    } catch {
      // The bus may already be closed by the failure path.
    }
  })
  return bus
}

test.serial('bus exposes a session id', async (t) => {
  const bus = await newBus(t)
  t.is(typeof bus.sessionId(), 'string')
  t.true(bus.sessionId().length > 0)
})

// Publishing to sub.topic is deliberate: the getter must not block on the
// receive that is already in flight. The raw getter used to take a lock the
// pending recv held, stalling the event loop until the recv timed out and
// the publish was dropped.
test.serial('publishText reaches a subscriber', async (t) => {
  const bus = await newBus(t)
  const sub = await bus.createSubscriber(topic('text'))
  const receiving = sub.recvWithTimeoutMs(5000)
  await sleep(200)
  await bus.publishText(sub.topic, 'hello bus')
  t.is(await receiving, 'hello bus')
})

test.serial('publishJson reaches a subscriber as a value', async (t) => {
  const bus = await newBus(t)
  const sub = await bus.createSubscriber(topic('json'))
  const receiving = sub.recvJsonWithTimeoutMs(5000)
  await sleep(200)
  await bus.publishJson(sub.topic, { kind: 'greeting', n: 2 })
  const value = await receiving
  t.is(value.kind, 'greeting')
  t.is(value.n, 2)
})

test.serial('publisher and subscriber objects round-trip', async (t) => {
  const bus = await newBus(t)
  const name = topic('objects')
  const pub = await bus.createPublisher(name)
  const sub = await bus.createSubscriber(name)
  t.is(pub.topic, name)
  t.is(sub.topic, name)
  const receiving = sub.recvWithTimeoutMs(5000)
  await sleep(200)
  await pub.publishText('from publisher')
  t.is(await receiving, 'from publisher')
})

test.serial('subscriber yields null when nothing arrives before the timeout', async (t) => {
  const bus = await newBus(t)
  const sub = await bus.createSubscriber(topic('empty'))
  t.is(await sub.recvWithTimeoutMs(150), null)
})