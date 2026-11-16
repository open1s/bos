#!/usr/bin/env node
/**
 * Async Hooks & Plugins Demo
 *
 * Demonstrates:
 * 1. Async hooks that perform async operations (simulated DB/API calls)
 * 2. Async plugins that intercept LLM requests/responses with async logic
 * 3. Hook decisions based on async results (abort, continue, error)
 * 4. Plugin modifications based on async results
 *
 * Usage:
 *     node crates/jsbos/examples/async_hooks_plugins_demo.js
 */

import {
  Agent,
  HookEvent,
  ConfigLoader,
} from '../index.js'

async function loadConfig() {
  const loader = new ConfigLoader()
  loader.discover()
  const cfg = JSON.parse(loader.loadSync())
  const agentCfg = cfg.agent || {}
  const globalCfg = cfg.global_model || {}

  return {
    name: agentCfg.name || 'async-demo',
    model: process.env.LLM_MODEL || globalCfg.model || 'nvidia/meta/llama-3.1-8b-instruct',
    apiKey: globalCfg.api_key || '',
    baseUrl: process.env.LLM_BASE_URL || globalCfg.base_url || 'https://integrate.api.nvidia.com/v1',
    systemPrompt: agentCfg.system_prompt || 'You are a helpful assistant.',
    temperature: agentCfg.temperature ?? 0.7,
    timeoutSecs: agentCfg.timeout_secs ?? 120,
  }
}

// ─── Simulated async operations ─────────────────────────────────────────────// ─── Simulated async operations ─────────────────────────────────────────────

async function checkRateLimit(userId) {
  await new Promise(resolve => setTimeout(resolve, 50))
  return { allowed: true, remaining: 99 }
}

async function logToExternalService(event, data) {
  await new Promise(resolve => setTimeout(resolve, 30))
  return { logged: true, eventId: `evt_${Date.now()}` }
}

async function enrichRequest(request) {
  await new Promise(resolve => setTimeout(resolve, 40))
  return {
    ...request,
    metadata: {
      ...(request.metadata || {}),
      enriched: true,
      timestamp: new Date().toISOString(),
    },
  }
}

async function analyzeResponse(response) {
  await new Promise(resolve => setTimeout(resolve, 30))
  return {
    ...response,
    response_type: response.response_type || response.type || 'Text',
    metadata: {
      ...(response.metadata || {}),
      analyzed: true,
      sentiment: 'positive',
    },
  }
}

async function main() {
  console.log('\n' + '═'.repeat(60))
  console.log('  BrainOS — Async Hooks & Plugins Demo')
  console.log('═'.repeat(60))

  const config = await loadConfig()
  console.log(`\nModel: ${config.model}`)

  const agent = await Agent.create(config)

  // ═══════════════════════════════════════════════════════════════════════════
  // ASYNC HOOKS
  // ═══════════════════════════════════════════════════════════════════════════

  console.log('\n' + '─'.repeat(60))
  console.log('  Step 1 — Registering Async Hooks')
  console.log('─'.repeat(60))

  await agent.registerHook(HookEvent.BeforeLlmCall, async (ctx) => {
    const rateLimit = await checkRateLimit('user-123')
    console.log(`  ⏳ [Async Hook:BeforeLlmCall] Rate limit: ${rateLimit.remaining} remaining`)
    if (!rateLimit.allowed) {
      console.log('  🚫 Rate limit exceeded, aborting!')
      return 'Abort'
    }
    return 'Continue'
  })

  await agent.registerHook(HookEvent.AfterLlmCall, async (ctx) => {
    const logResult = await logToExternalService('llm_response', ctx)
    console.log(`  ⏳ [Async Hook:AfterLlmCall] Logged: ${logResult.eventId}`)
    return 'Continue'
  })

  await agent.registerHook(HookEvent.OnMessage, async (ctx) => {
    const logResult = await logToExternalService('message', ctx)
    console.log(`  ⏳ [Async Hook:OnMessage] Logged: ${logResult.eventId}`)
    return 'Continue'
  })

  await agent.registerHook(HookEvent.OnComplete, async (ctx) => {
    const logResult = await logToExternalService('complete', ctx)
    console.log(`  ⏳ [Async Hook:OnComplete] Logged: ${logResult.eventId}`)
    return 'Continue'
  })

  await agent.registerHook(HookEvent.OnError, async (ctx) => {
    const logResult = await logToExternalService('error', ctx)
    console.log(`  ⏳ [Async Hook:OnError] Logged: ${logResult.eventId}`)
    return 'Continue'
  })

  console.log('  ✅ 5 async hooks registered')

  // ═══════════════════════════════════════════════════════════════════════════
  // ASYNC PLUGINS
  // ═══════════════════════════════════════════════════════════════════════════

  console.log('\n' + '─'.repeat(60))
  console.log('  Step 2 — Registering Async Plugins')
  console.log('─'.repeat(60))

  agent.registerPlugin(
    'AsyncRequestEnricher',
    async (err, request) => {
      if (err) return JSON.stringify(request)
      console.log(`  ⏳ [Async Plugin:on_llm_request] Enriching request for model=${request.model}`)
      const enriched = await enrichRequest(request)
      console.log(`  ✅ Enriched with metadata: ${JSON.stringify(enriched.metadata)}`)
      return JSON.stringify(enriched)
    },
    async (err, response) => {
      if (err) return JSON.stringify(response)
      const respType = response.response_type || 'Text'
      console.log(`  ⏳ [Async Plugin:on_llm_response] Analyzing response type=${respType}`)
      const analyzed = await analyzeResponse(response)
      console.log(`  ✅ Analyzed with sentiment: positive`)
      // Return modified response (plugin can modify content)
      return JSON.stringify(analyzed)
    },
    null,
    null,
  )

  console.log('  ✅ Async plugin registered (on_llm_request + on_llm_response)')

  // ═══════════════════════════════════════════════════════════════════════════
  // TEST RUNS
  // ═══════════════════════════════════════════════════════════════════════════

  console.log('\n' + '─'.repeat(60))
  console.log('  Step 3 — Running Tests')
  console.log('─'.repeat(60))

  const tests = [
    ['Simple Question', 'What is the capital of France?'],
    ['Math', 'What is 42 + 58?'],
    ['Creative', 'Write a haiku about async code.'],
  ]

  for (const [label, prompt] of tests) {
    console.log(`\n  [${label}] User: ${prompt}`)
    try {
      const result = await agent.runSimple(prompt)
      console.log(`  [${label}] Agent: ${result}`)
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`)
    }
  }

  console.log('\n' + '═'.repeat(60))
  console.log('  ✅ Async Hooks & Plugins Demo completed!')
  console.log('═'.repeat(60) + '\n')

  agent.close()
}

main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(err)
    process.exit(1)
  })
