#!/usr/bin/env node
/**
 * Session Management Demo
 *
 * Demonstrates:
 * 1. Save/restore session to/from JSON
 * 2. Save/restore session to/from file
 * 3. Clear session (keeps system messages)
 * 4. Compact session (summarize old messages)
 * 5. Export session
 *
 * Usage: node session_demo.js
 */

import { Bus, Agent, ConfigLoader, version, initTracing } from '../index.js'
import fs from 'fs'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const config = JSON.parse(loader.loadSync())
const global = config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || global.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || global.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || global.model || 'nvidia/z-ai/glm4.7'

async function main() {
  console.log('\n' + '📋'.repeat(30))
  console.log('  BrainOS — Session Management Demo')
  console.log('📋'.repeat(30) + '\n')

  const bus = await Bus.create()
  console.log('  🚌 Bus created')

  if (!API_KEY) {
    console.log('  ⚠️  No API key — set OPENAI_API_KEY or config.toml')
    process.exit(1)
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a helpful assistant with a quirky personality.',
    temperature: 0.7,
    timeoutSecs: 60,
  }, bus)
  console.log('  🤖 Agent created\n')

  console.log('═'.repeat(60))
  console.log('  Step 1 — Generate conversation')
  console.log('═'.repeat(60))

  const q1 = await agent.runSimple('Say hello and tell me a fun fact')
  console.log('  Q1:', q1.substring(0, 100) + '...\n')

  const q2 = await agent.runSimple('What is Python?')
  console.log('  Q2:', q2.substring(0, 100) + '...\n')

  console.log('═'.repeat(60))
  console.log('  Step 2 — Get session as JSON')
  console.log('═'.repeat(60))

  const sessionJson = agent.getSessionJson()
  const session = JSON.parse(sessionJson)
  console.log('  Session has', session.messages?.length || 0, 'messages')
  console.log('  Context keys:', Object.keys(session.context || {}).join(', ') || '(none)')

  console.log('═'.repeat(60))
  console.log('  Step 3 — Export session')
  console.log('═'.repeat(60))

  const exported = agent.exportSession()
  console.log('  Exported JSON length:', exported.length, 'chars\n')

  console.log('═'.repeat(60))
  console.log('  Step 4 — Save session to file')
  console.log('═'.repeat(60))

  const sessionPath = '/tmp/bos_session_demo.json'
  agent.saveSession(sessionPath)
  console.log('  ✅ Saved to:', sessionPath)
  const savedContent = fs.readFileSync(sessionPath, 'utf8')
  console.log('  File size:', savedContent.length, 'chars\n')

  console.log('═'.repeat(60))
  console.log('  Step 5 — Add more messages then restore')
  console.log('═'.repeat(60))

  const q3 = await agent.runSimple('What is Rust?')
  console.log('  Added Q3')
  const jsonBeforeRestore = agent.getSessionJson()
  const sessionBefore = JSON.parse(jsonBeforeRestore)
  console.log('  Messages before restore:', sessionBefore.messages?.length)

  agent.restoreSessionJson(sessionJson)
  const jsonAfterRestore = agent.getSessionJson()
  const sessionAfter = JSON.parse(jsonAfterRestore)
  console.log('  Messages after restore:', sessionAfter.messages?.length)

  console.log('═'.repeat(60))
  console.log('  Step 6 — Restore from file')
  console.log('═'.repeat(60))

  const q4 = await agent.runSimple('What is JavaScript?')
  console.log('  Added Q4')
  const beforeFile = agent.getSessionJson()
  console.log('  Messages before file restore:', JSON.parse(beforeFile).messages?.length)

  agent.restoreSessionFromFile(sessionPath)
  const afterFile = agent.getSessionJson()
  console.log('  Messages after file restore:', JSON.parse(afterFile).messages?.length, '(should be 2)\n')

  console.log('═'.repeat(60))
  console.log('  Step 7 — Clear session (keeps system message)')
  console.log('═'.repeat(60))

  const beforeClear = agent.getSessionJson()
  console.log('  Messages before clear:', JSON.parse(beforeClear).messages?.length)

  agent.clearSession()

  const afterClear = JSON.parse(agent.getSessionJson())
  console.log('  Messages after clear:', afterClear.messages?.length)
  const systemMessages = afterClear.messages?.filter(m => m.role === 'system')
  console.log('  System messages preserved:', systemMessages?.length > 0 ? 'Yes' : 'No')

  console.log('═'.repeat(60))
  console.log('  Step 8 — Compact session')
  console.log('═'.repeat(60))

  await agent.runSimple('Tell me about AI')
  await agent.runSimple('Tell me about machine learning')
  await agent.runSimple('What is deep learning?')

  const beforeCompact = JSON.parse(agent.getSessionJson())
  console.log('  Messages before compact:', beforeCompact.messages?.length)

  agent.compactSession(2, 500)

  const afterCompact = JSON.parse(agent.getSessionJson())
  console.log('  Messages after compact (keep_recent=2):', afterCompact.messages?.length)
  if (afterCompact.context?.compacted_summary) {
    console.log('  Summary created:', afterCompact.context.compacted_summary.substring(0, 80) + '...')
  }

  console.log('\n' + '═'.repeat(60))
  console.log('  ✅ Session management demo completed!')
  console.log('═'.repeat(60) + '\n')

  process.exit(0)
}

main().catch(e => { console.error('Error:', e.message); process.exit(1) })