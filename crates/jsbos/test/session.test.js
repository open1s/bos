import test from 'ava'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { mkdtempSync, rmSync } from 'node:fs'
import { BrainOS, ToolDef, HookEvent } from '../index.js'

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

test.serial('setContext and clearContext round-trip without touching messages', async (t) => {
  const agent = await startAgent(t)
  agent.session.addMessage('user', 'keep me')
  t.is(agent.session.context, null)

  agent.session.setContext({ topic: 'physics' })
  t.deepEqual(agent.session.context, { topic: 'physics' })

  agent.session.clearContext()
  t.is(agent.session.context, null)
  t.is(agent.session.getMessages().length, 1)
})

test.serial('clear drops messages and resets the context in both bindings', async (t) => {
  const agent = await startAgent(t)
  agent.session.addMessage('user', 'drop me')
  agent.session.setContext({ topic: 'trig' })

  agent.session.clear()
  t.is(agent.session.getMessages().length, 0)
  t.is(agent.session.context, null)
})

test.serial('config exposes the resolved agent config', async (t) => {
  const agent = await startAgent(t)
  t.is(agent.config.name, 'session-test')
  t.truthy(agent.config.model)
})

test.serial('toolNames lists registered tools', async (t) => {
  const brain = new BrainOS()
  await brain.start()
  t.teardown(() => brain.stop())
  const addTool = new ToolDef('add', 'Add two numbers', (args) => (args.a || 0) + (args.b || 0))
  const agent = await brain.agent('tools-test').register(addTool).start()
  t.deepEqual(agent.toolNames, ['add'])
  t.deepEqual(agent.session.context, null)
})

// A registered callback must not pin the Node event loop: if it did, ava would
// fail to exit after the suite. These two tests would hang the process.
test.serial('registering a plugin does not pin the event loop', async (t) => {
  const brain = new BrainOS()
  await brain.start()
  t.teardown(() => brain.stop())
  const agent = await brain.agent('plugin-exit-test')
    .plugin('noop', { on_llm_request: (d) => d })
    .start()
  t.is(agent.config.name, 'plugin-exit-test')
})

test.serial('registering a hook does not pin the event loop', async (t) => {
  const brain = new BrainOS()
  await brain.start()
  t.teardown(() => brain.stop())
  const agent = await brain.agent('hook-exit-test')
    .hook(HookEvent.BeforeToolCall, () => 'continue')
    .start()
  t.is(agent.config.name, 'hook-exit-test')
})

test.serial('metrics exposes a zeroed perf snapshot and resets', async (t) => {
  const agent = await startAgent(t)
  t.is(agent.metrics.llmCallCount, 0)
  t.is(agent.metrics.minWallTimeUs, 0)
  t.is(agent.metrics.maxWallTimeUs, 0)
  t.is(agent.resetMetrics(), agent)
})

test.serial('export/import round-trip through objects and JSON strings', async (t) => {
  const agent = await startAgent(t)
  agent.session.addMessage('user', 'snapshot me')
  const snapshot = agent.session.export()
  t.is(typeof snapshot, 'object')
  t.true(Array.isArray(snapshot.messages))
  t.is(typeof agent.session.exportJson(), 'string')
  agent.session.addMessage('user', 'another')
  agent.session.importSession(snapshot)
  t.deepEqual(agent.session.getMessages(), snapshot.messages)
  agent.session.import(JSON.stringify(snapshot))
  t.deepEqual(agent.session.getMessages(), snapshot.messages)
})

test.serial('high-level agent exposes empty MCP listings without servers', async (t) => {
  const agent = await startAgent(t)
  t.deepEqual(await agent.listMcpTools(), [])
  t.deepEqual(await agent.listMcpResources('none'), [])
  t.deepEqual(await agent.listMcpPrompts(), [])
})

test.serial('stop suppresses the next call and reports run state', async (t) => {
  const agent = await startAgent(t)
  t.false(agent.isRunning())
  t.false(agent.stop().stopped)
  t.is(await agent.runSimple('hi'), '')
})

test.serial('builder aliases chain and expose the mirrored surface', async (t) => {
  const agent = await startAgent(t)
  t.is(typeof agent.chat, 'function')
  t.is(agent.plugins({ name: 'parity' }), agent)
  t.is(agent.skillsDir('/tmp/parity-skills'), agent)
})

// CI has no ~/.bos/conf/config.toml, so agent construction must not depend on
// a config file providing the API key.
test.serial('BrainOS builds an agent with no config file or api key', async (t) => {
  const home = mkdtempSync(join(tmpdir(), 'bos-home-'))
  t.teardown(() => rmSync(home, { recursive: true, force: true }))
  const prevHome = process.env.HOME
  const prevKey = process.env.OPENAI_API_KEY
  process.env.HOME = home
  delete process.env.OPENAI_API_KEY
  t.teardown(() => {
    process.env.HOME = prevHome
    if (prevKey === undefined) delete process.env.OPENAI_API_KEY
    else process.env.OPENAI_API_KEY = prevKey
  })

  const brain = new BrainOS()
  await brain.start()
  t.teardown(() => brain.stop())
  const agent = await brain.agent('no-key-test').start()
  t.is(agent.config.name, 'no-key-test')
})

test.serial('BrainOS reads OPENAI_API_KEY from the environment', async (t) => {
  const prev = process.env.OPENAI_API_KEY
  process.env.OPENAI_API_KEY = 'env-key'
  t.teardown(() => {
    if (prev === undefined) delete process.env.OPENAI_API_KEY
    else process.env.OPENAI_API_KEY = prev
  })

  const brain = new BrainOS()
  await brain.start()
  t.teardown(() => brain.stop())
  const builder = brain.agent('env-key-test')
  t.is(builder._config.apiKey, 'env-key')
  const agent = await builder.start()
  t.is(agent.config.name, 'env-key-test')
})
