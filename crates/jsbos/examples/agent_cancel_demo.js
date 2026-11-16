#!/usr/bin/env node
/**
 * Agent Cancel Demo — Two real cancellation scenarios.
 *
 * Scenario A: External bus cancel — a separate watcher subscribes to
 *   lifecycle events and publishes cancel on the bus after observing
 *   bashOp start. This mimics an operator/control-plane cancelling a
 *   long-running background task externally.
 *
 * Scenario B: Two-agent orchestration — a "controller" subscriber watches
 *   the worker agent's tool events, and when it sees bashOp running, it
 *   publishes cancel. This shows cross-agent coordination where one
 *   component manages another's execution.
 *
 * Both scenarios test the full path:
 *   bus publish -> engine ToolRunMgr -> tool.cancel(call_id) ->
 *   JSTool.cancel_callback(call_id) -> user cancel handler ->
 *   child.kill(SIGTERM) -> background HTTP server exits.
 *
 * Note: napi-rs keeps 2 permanent libuv Socket handles (tokio runtime I/O
 * driver), so process.exit(0) is required at the end for the process to
 * terminate. See crates/jsbos for details.
 *
 * Usage:
 *     node crates/jsbos/examples/agent_cancel_demo.js
 *     RUST_LOG=debug node crates/jsbos/examples/agent_cancel_demo.js
 *
 * Requires: OPENAI_API_KEY (or ~/.bos/conf/config.toml)
 */

import { Bus, Agent, ConfigLoader, initTracing } from '../index.js'
import { spawn } from 'child_process'

const loader = new ConfigLoader()
loader.discover()
const raw = JSON.parse(loader.loadSync())
const glob = raw.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || glob.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || glob.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || glob.model || 'nvidia/meta/llama-3.1-8b-instruct'

// --- Shared tool definition ------------------------------------------------

const SLOW_OP_SCHEMA = {
  type: 'object',
  properties: {
    key: { type: 'string', description: 'A key to process (~7s)' },
  },
  required: ['key'],
}

/**
 * Creates a cancelable tool that internally spawns a background HTTP server
 * as a child process. The server listens on a dynamic port and stays alive
 * until cancel kills it via SIGTERM — demonstrating real process lifecycle
 * management through the cancellation API.
 *
 * The LLM sees a harmless "slow operation" and calls it normally.
 * The demo's console output reveals the actual child-process lifecycle.
 *
 * Returns { run, cancel } where cancel is the cancel callback.
 */
function createBashOp() {
  const processes = new Map()

  const run = async (args) => {
    const key = args?.key ?? 'unknown'
    const callId = args?.__call_id__ ?? 'unknown'

    console.log(`  [bashOp] start  call_id=${callId}  key=${key}`)
    const start = Date.now()

    // Spawn a background HTTP server. `detached: true` puts it in its own
    // process group (like `command &` in shell), so the cancel handler can
    // kill the entire group via `process.kill(-pid, SIGTERM)` — the shell-
    // background equivalent of `kill -- -$(pgid)`.
    const child = spawn(process.execPath, ['-e', `
      var h=require('http');
      h.createServer(function(q,r){r.writeHead(200,{'Content-Type':'text/plain'});r.end('ok\\n');})
       .listen(0,function(){process.stderr.write('PORT:'+this.address().port+'\\n');});
      process.on('SIGTERM',function(){process.exit(0);});
    `], { detached: true, stdio: ['ignore', 'inherit', 'pipe'] })

    processes.set(callId, child)
    console.log(`  [bashOp] spawned server pid=${child.pid} (group)`)

    let port = 'unknown'
    child.stderr.on('data', (d) => {
      const m = d.toString().match(/PORT:(\d+)/)
      if (m) port = m[1]
    })

    return new Promise((resolve) => {
      child.on('exit', (code, signal) => {
        const result = signal ? 'killed' : 'done'
        const elapsed = ((Date.now() - start) / 1000).toFixed(1)
        console.log(`  [bashOp] end    call_id=${callId}  result=${result}  (${elapsed}s) port=${port}`)
        processes.delete(callId)
        resolve({ key, call_id: callId, result, port })
      })
    })
  }

  const cancel = (_err, callId) => {
    console.log(`  [bashOp] cancel call_id=${callId}`)
    const child = processes.get(callId)
    if (child) {
      // Kill the entire process group — this is the correct way to kill a
      // shell-backgrounded job: `kill -- -$(pgid)` or `kill $(jobs -p)`.
      console.log(`  [bashOp] killing process group -${child.pid}`)
      try {
        process.kill(-child.pid, 'SIGTERM')
      } catch {
        child.kill('SIGTERM')
      }
    } else {
      console.log(`  [bashOp] no active process for call_id=${callId}`)
    }
  }

  return { run, cancel }
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms))
}

// ---- Scenario A: External watcher cancels a running tool ------------------

async function scenarioA_ExternalWatcherCancel() {
  console.log('')
  console.log('='.repeat(60))
  console.log('  Scenario A: External bus-watcher cancels the agent')
  console.log('='.repeat(60))
  console.log('')

  const bus = await Bus.create()
  const session = await bus.session()

  const agentName = 'cancel-demo-a'
  const cancelTopic = `agent/${agentName}/tool/cancel`
  const eventsTopic = `agent/${agentName}/tool/events`
  console.log(`[bus] cancel topic:  ${cancelTopic}`)
  console.log(`[bus] events topic:  ${eventsTopic}`)

  const agent = await Agent.createWithBus(
    {
      name: agentName,
      model: MODEL,
      baseUrl: BASE_URL,
      apiKey: API_KEY,
      temperature: 0.2,
      timeoutSecs: 60,
      systemPrompt:
        'You are a test assistant. Use the slowOp tool exactly once ' +
        'with key "scenario-A", then tell me the result.',
    },
    session,
  )

  const { run, cancel } = createBashOp()
  const callback = (err, args) => {
    if (err) return Promise.resolve({ error: String(err) })
    return run(args)
  }
  await agent.addTool(
    'slowOp',
    'A slow ~7s operation. Cancelable.',
    JSON.stringify(SLOW_OP_SCHEMA.properties),
    JSON.stringify(SLOW_OP_SCHEMA),
    callback,
    true,
    cancel
  )
  console.log('[ok] slowOp registered as cancelable\n')

  // Watcher: subscribes to events, sees "started", then cancels.
  const eventsSub = await bus.createSubscriber(eventsTopic)
  let startedId = null
  const startedPromise = new Promise((resolve) => {
    eventsSub.runJson((err, event) => {
      if (err) return
      const s = event || {}
      console.log(`  [watcher] ${s.status}: ${s.tool} (call_id=${s.call_id})`)
      if (s.status === 'started' && !startedId) {
        startedId = s.call_id
        resolve()
      }
    }).catch(() => {})
  })

  const askPromise = agent
    .runSimple('Use slowOp with key "scenario-A". Summarize the result.')
    .catch((e) => ({ error: String(e) }))

  // Wait for slowOp to start (up to 60s for LLM), then publish cancel.
  try {
    await Promise.race([startedPromise, sleep(60000)])
    if (startedId) {
      console.log(`\n[watcher] publishing cancel: call_id=${startedId}`)
      await bus.publishJson(cancelTopic, { call_id: startedId })
    }
  } catch {}
  if (!startedId) {
    console.log('\n[watcher] no started call_id seen — slowOp may complete on its own\n')
  }

  const result = await askPromise
  console.log('\n--- Agent final response ---')
  console.log(typeof result === 'string' ? result : JSON.stringify(result))

  agent.close()
  await eventsSub.stop()
  await bus.close()
  console.log('\n' + '-'.repeat(40))
  console.log('Scenario A complete.')
  console.log('Path: events sub -> external bus cancel publish ->')
  console.log('      engine ToolRunMgr -> tool.cancel(call_id) -> child.kill(SIGTERM)')
  console.log('-'.repeat(40))
}

// ---- Scenario B: Controller cancels worker agent --------------------------

async function scenarioB_TwoAgentOrchestration() {
  console.log('')
  console.log('='.repeat(60))
  console.log('  Scenario B: Controller cancels worker agent')
  console.log('='.repeat(60))
  console.log('')

  const bus = await Bus.create()
  const session = await bus.session()

  // ---- Worker agent ----
  const workerName = 'worker-agent'
  const cancelTopic = `agent/${workerName}/tool/cancel`
  const eventsTopic = `agent/${workerName}/tool/events`
  console.log(`[bus] worker cancel:  ${cancelTopic}`)
  console.log(`[bus] worker events:  ${eventsTopic}\n`)

  const worker = await Agent.createWithBus(
    {
      name: workerName,
      model: MODEL,
      baseUrl: BASE_URL,
      apiKey: API_KEY,
      temperature: 0.2,
      timeoutSecs: 60,
      systemPrompt:
        'You are a worker. Use the slowOp tool exactly once ' +
        'with key "scenario-B", then report the result.',
    },
    session,
  )

  const { run: wr, cancel: wc } = createBashOp()
  const wCallback = (err, args) => {
    if (err) return Promise.resolve({ error: String(err) })
    return wr(args)
  }
  await worker.addTool(
    'slowOp',
    'A slow ~7s operation. Cancelable.',
    JSON.stringify(SLOW_OP_SCHEMA.properties),
    JSON.stringify(SLOW_OP_SCHEMA),
    wCallback,
    true,
    wc
  )
  console.log('[ok] worker slowOp registered\n')

  // ---- Controller subscriber ----
  // A policy engine, anomaly detector, or human operator could observe
  // tool events and decide to cancel. Here we simulate: slowOp running
  // too long, cancel because processing budget is exceeded.
  let workerCallId = null
  const eventsSub = await bus.createSubscriber(eventsTopic)
  const startedPromise = new Promise((resolve) => {
    eventsSub.runJson((err, event) => {
      if (err) return
      const s = event || {}
      console.log(`  [controller] event: ${s.status} ${s.tool} (call_id=${s.call_id})`)
      if (s.status === 'started' && !workerCallId) {
        workerCallId = s.call_id
        resolve()
      }
    }).catch(() => {})
  })

  // Kick off the worker.
  const workerTask = worker
    .runSimple('Use slowOp with key "scenario-B". Summarize the result.')
    .catch((e) => ({ error: String(e) }))

  // Controller waits for started event, then cancels.
  console.log('[controller] waiting for worker to start slowOp...\n')
  try {
    await Promise.race([startedPromise, sleep(60000)])
    if (workerCallId) {
      console.log(`\n[controller] cancelling worker call_id=${workerCallId}`)
      console.log('[controller] reason: processing budget exceeded for scenario-B')
      await bus.publishJson(cancelTopic, { call_id: workerCallId })
    }
  } catch {}
  if (!workerCallId) {
    console.log('\n[controller] never saw started event — skip cancel\n')
  }

  const workerResult = await workerTask
  console.log('\n--- Worker final response ---')
  console.log(typeof workerResult === 'string' ? workerResult : JSON.stringify(workerResult))

  worker.close()
  await eventsSub.stop()
  await bus.close()
  console.log('\n' + '-'.repeat(40))
  console.log('Scenario B complete.')
  console.log('  Subscriber watches worker events,')
  console.log('  decides to cancel based on policy (time budget),')
  console.log('  publishes cancel -> child.kill(SIGTERM) -> server exits.')
  console.log('-'.repeat(40))
}

// ---- Main -----------------------------------------------------------------

async function main() {
  console.log('')
  console.log('='.repeat(60))
  console.log('   BrainOS Agent Cancel Demo — Two Scenarios')
  console.log('='.repeat(60))
  initTracing()

  if (!API_KEY) {
    console.log('\n[SKIP] No API key; set OPENAI_API_KEY or configure --/.bos/config.toml\n')
    return
  }

  console.log(`Model: ${MODEL}`)

  await scenarioA_ExternalWatcherCancel()
  await scenarioB_TwoAgentOrchestration()

  console.log('\nAll cancel demos finished.\n')
  process.exit(0) // napi-rs tokio runtime handles keep event loop alive
}

main().catch(console.error)