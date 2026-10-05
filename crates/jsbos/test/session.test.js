import test from 'ava'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { rmSync } from 'node:fs'
import { BrainOS } from '../index.js'

async function startAgent(t) {
  const brain = new BrainOS()
  await brain.start()
  t.teardown(() => brain.stop())
  return brain.agent('session-test').start()
}

test.serial('addMessage and getMessages round-trip', async (t) => {
  const agent = await startAgent(t)
  agent.session.addMessage('user', 'hello')
  const messages = agent.session.getMessages()
  t.true(Array.isArray(messages))
  t.is(messages.length, 1)
})

test.serial('save and restore a message log', async (t) => {
  const agent = await startAgent(t)
  const file = join(tmpdir(), 'jsbos-session-' + process.pid + '.json')
  t.teardown(() => rmSync(file, { force: true }))
  agent.session.addMessage('user', 'persist me')
  await agent.session.save(file)
  agent.session.clear()
  await agent.session.restore(file)
  const messages = agent.session.getMessages()
  t.true(Array.isArray(messages))
  t.is(messages.length, 1)
})

test.serial('compact forwards its limits', async (t) => {
  const agent = await startAgent(t)
  for (let i = 0; i < 30; i += 1) {
    agent.session.addMessage('user', 'message ' + i)
  }
  agent.session.compact(5, 100)
  t.true(Array.isArray(agent.session.getMessages()))
})

test.serial('saveFull and restoreFull round-trip', async (t) => {
  const agent = await startAgent(t)
  const file = join(tmpdir(), 'jsbos-session-full-' + process.pid + '.json')
  t.teardown(() => rmSync(file, { force: true }))
  agent.session.addMessage('user', 'full')
  agent.session.saveFull(file)
  agent.session.restoreFull(file)
  t.true(Array.isArray(agent.session.getMessages()))
})

test.serial('addMessage rejects an unsupported role', async (t) => {
  const agent = await startAgent(t)
  const error = t.throws(() => agent.session.addMessage('bogus', 'x'))
  t.regex(String(error.message), /unsupported message role/)
})

test.serial('context exposes the session context', async (t) => {
  const agent = await startAgent(t)
  t.is(agent.session.context, null)
  agent.session.import(
    JSON.stringify({
      context: { topic: 'math' },
      messages: [],
      metadata: { created_at: 0, updated_at: 0, message_count: 0 },
    })
  )
  t.deepEqual(agent.session.context, { topic: 'math' })
})

test.serial('config exposes the resolved agent config', async (t) => {
  const agent = await startAgent(t)
  t.is(agent.config().name, 'session-test')
  t.truthy(agent.config().model)
})
