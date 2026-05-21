#!/usr/bin/env node
/**
 * Async Tool Calling Demo
 *
 * Demonstrates:
 * 1. Registering sync tools with addTool()
 * 2. Registering async tools with addTool() (auto-detects Promise)
 * 3. LLM using both sync and async tools in the same conversation
 *
 * Async tools are useful when your tool needs to:
 *  - Make HTTP requests (fetch, axios)
 *  - Read/write files (fs.promises)
 *  - Query databases
 *  - Call external APIs with delays
 *
 * Usage:
 *     node crates/jsbos/examples/async_tool_demo.js
 */

import { Bus, Agent, ConfigLoader, initTracing } from '../index.js'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const _config = JSON.parse(loader.loadSync())
const _global = _config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct'

// ─── Sync Tools (use addTool) ───────────────────────────────────────────────

function calculatorTool(args) {
  const expr = args.expression || ''
  const allowed = new Set('0123456789+-*/.() ')
  if (![...expr].every(c => allowed.has(c))) {
    return JSON.stringify({ error: 'Invalid characters in expression' })
  }
  try {
    const result = eval(expr)
    return JSON.stringify({ expression: expr, result })
  } catch (e) {
    return JSON.stringify({ error: e.message })
  }
}

const CALCULATOR_SCHEMA = {
  type: 'object',
  properties: {
    expression: {
      type: 'string',
      description: "A math expression to evaluate, e.g. '2 + 3 * 4'",
    },
  },
  required: ['expression'],
}

// ─── Async Tools (use addAsyncTool) ─────────────────────────────────────────

async function fetchWeather(args) {
  const city = args.city || 'unknown'
  console.log(`  ⏳ [async] Fetching weather for: ${city}...`)

  // Simulate network delay (replace with real fetch in production)
  await new Promise(resolve => setTimeout(resolve, 300))

  const mockData = {
    city,
    temperature: Math.floor(Math.random() * 35) - 5,
    unit: '°C',
    condition: ['sunny', 'cloudy', 'rainy', 'windy'][Math.floor(Math.random() * 4)],
    humidity: Math.floor(Math.random() * 60) + 30,
    fetched_at: new Date().toISOString(),
  }
  return JSON.stringify(mockData)
}

const WEATHER_SCHEMA = {
  type: 'object',
  properties: {
    city: {
      type: 'string',
      description: "City name, e.g. 'Tokyo', 'London', 'New York'",
    },
  },
  required: ['city'],
}

async function fetchUser(args) {
  const userId = args.user_id || ''
  console.log(`  ⏳ [async] Looking up user: ${userId}...`)

  // Simulate database query delay
  await new Promise(resolve => setTimeout(resolve, 200))

  const users = {
    'u1': { id: 'u1', name: 'Alice Chen', role: 'admin', email: 'alice@example.com' },
    'u2': { id: 'u2', name: 'Bob Smith', role: 'editor', email: 'bob@example.com' },
    'u3': { id: 'u3', name: 'Carol Wu', role: 'viewer', email: 'carol@example.com' },
  }

  const user = users[userId]
  if (!user) {
    return JSON.stringify({ error: `User '${userId}' not found` })
  }
  return JSON.stringify(user)
}

const USER_SCHEMA = {
  type: 'object',
  properties: {
    user_id: {
      type: 'string',
      description: "User ID to look up, e.g. 'u1', 'u2', 'u3'",
    },
  },
  required: ['user_id'],
}

async function sendNotification(args) {
  const { to, message } = args
  console.log(`  ⏳ [async] Sending notification to ${to}...`)

  // Simulate sending delay
  await new Promise(resolve => setTimeout(resolve, 150))

  return JSON.stringify({
    sent: true,
    to,
    message_id: `msg_${Date.now()}`,
    timestamp: new Date().toISOString(),
  })
}

const NOTIFY_SCHEMA = {
  type: 'object',
  properties: {
    to: {
      type: 'string',
      description: "Recipient email or username",
    },
    message: {
      type: 'string',
      description: "Notification message content",
    },
  },
  required: ['to', 'message'],
}

async function main() {
  console.log('\n' + '═'.repeat(60))
  console.log('  BrainOS — Async Tool Calling Demo')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  console.log('  🚌 Bus created')

  if (!API_KEY) {
    console.log('  ⚠️  No API key found — set OPENAI_API_KEY or create ~/.bos/conf/config.toml')
    console.log('  Skipping LLM demo\n')
    return
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a helpful assistant. ' +
      'Use the available tools when they can help answer the question. ' +
      'Format: Thought: <reasoning>\nFinal Answer: <response or tool call>',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus)

  console.log('\n' + '─'.repeat(60))
  console.log('  Step 1 — Registering Sync Tools (addTool)')
  console.log('─'.repeat(60))

  await agent.addTool(
    'calculator',
    'Evaluate a mathematical expression and return the result.',
    JSON.stringify(CALCULATOR_SCHEMA.properties),
    JSON.stringify(CALCULATOR_SCHEMA),
    (err, args) => calculatorTool(args),
  )
  console.log('  ✅ Registered sync tool: calculator')

  console.log('\n' + '─'.repeat(60))
  console.log('  Step 2 — Registering Async Tools (addTool auto-detects async)')
  console.log('─'.repeat(60))

  await agent.addTool(
    'weather',
    'Get current weather information for a given city. Simulates an API call.',
    JSON.stringify(WEATHER_SCHEMA.properties),
    JSON.stringify(WEATHER_SCHEMA),
    async (err, args) => fetchWeather(args),
  )
  console.log('  ✅ Registered async tool: weather')

  await agent.addTool(
    'lookup_user',
    'Look up a user profile by their user ID. Simulates a database query.',
    JSON.stringify(USER_SCHEMA.properties),
    JSON.stringify(USER_SCHEMA),
    async (err, args) => fetchUser(args),
  )
  console.log('  ✅ Registered async tool: lookup_user')

  await agent.addTool(
    'send_notification',
    'Send a notification message to a recipient. Simulates an email/push notification.',
    JSON.stringify(NOTIFY_SCHEMA.properties),
    JSON.stringify(NOTIFY_SCHEMA),
    async (err, args) => sendNotification(args),
  )
  console.log('  ✅ Registered async tool: send_notification')

  console.log(`\n  All tools: ${agent.listTools()}`)
  console.log(`  Async tools: ${agent.listAsyncTools()}`)

  console.log('\n' + '─'.repeat(60))
  console.log('  Step 3 — Agent Using Both Sync & Async Tools')
  console.log('─'.repeat(60))

  const prompts = [
    ['Sync Math', 'What is 2468 * 1357?'],
    ['Async Weather', "What's the weather like in Tokyo right now?"],
    ['Async Lookup', 'Look up user u1 and tell me their name and role.'],
    ['Async Lookup (missing)', 'Look up user u999.'],
    ['Async Notify', 'Send a notification to alice@example.com saying "Your account has been updated."'],
    ['Mixed', 'Calculate 100 * 3.14, then check the weather in London and send the result to bob@example.com.'],
  ]

  for (const [label, prompt] of prompts) {
    console.log(`\n  [${label}] User: ${prompt}`)
    try {
      const reply = await agent.runSimple(prompt)
      console.log(`  [${label}] Agent: ${reply}`)
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`)
    }
  }

  console.log('\n' + '═'.repeat(60))
  console.log('  ✅ Async Tool Demo completed!')
  console.log('═'.repeat(60) + '\n')
}

main().catch(console.error).finally(() => process.exit(0))
