// A run holds the agent lock for the whole LLM call. Read-only getters take a
// shared read lock, so they must stay responsive while a run is in flight; they
// used to call blocking_lock and freeze the Node event loop for the whole run.
import test from 'ava'
import { createServer } from 'node:http'
import { Agent } from '../index.js'

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

// A server that signals when a request arrives and answers slowly, so react
// stays in flight while the test probes the agent.
function slowLlmServer(delayMs) {
  let notify
  const requestSeen = new Promise((resolve) => {
    notify = resolve
  })
  const server = createServer((req, res) => {
    notify()
    setTimeout(() => {
      res.writeHead(200, { 'content-type': 'application/json' })
      res.end(
        JSON.stringify({
          id: 'chatcmpl-test',
          object: 'chat.completion',
          created: 0,
          model: 'test',
          choices: [{ index: 0, message: { role: 'assistant', content: 'ok' }, finish_reason: 'stop' }],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
      )
    }, delayMs)
  })
  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve({ server, requestSeen, port: server.address().port }))
  })
}

test.serial('read-only getters stay responsive while a run is in flight', async (t) => {
  const { server, requestSeen, port } = await slowLlmServer(2500)
  t.teardown(() => server.close())

  const agent = await Agent.create({
    name: 'concurrency-test',
    model: 'test',
    baseUrl: 'http://127.0.0.1:' + port + '/v1',
    apiKey: 'test-key',
    systemPrompt: 'You are a test.',
    temperature: 0.0,
    timeoutSecs: 30,
  })
  t.teardown(() => agent.close())

  // The run takes the lock before it sends the request, so once the server has
  // seen the request the lock is held and a getter would block without a fix.
  const running = agent.react('hello').catch((err) => String(err))
  await Promise.race([
    requestSeen,
    sleep(5000).then(() => {
      throw new Error('agent never reached the mock LLM endpoint')
    }),
  ])

  const started = Date.now()
  const config = agent.config()
  const elapsed = Date.now() - started
  t.is(config.name, 'concurrency-test')
  t.true(elapsed < 1000, 'config blocked for ' + elapsed + 'ms while a run was in flight')

  agent.stop()
  await running
})