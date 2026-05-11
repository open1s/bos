#!/usr/bin/env node
/**
 * BrainOS Wrapper Demo — High-level API
 *
 * Demonstrates:
 * 1. BrainOS.start() for lifecycle management
 * 2. brainos.agent() for creating agents
 * 3. agent.ask() for simple queries
 *
 * Usage:
 *     node crates/jsbos/examples/brainos_demo.js
 */

import { BrainOS, version } from '../index.js'

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

async function demoBasicUsage() {
  console.log('═'.repeat(60))
  console.log('  Demo 1 — Basic BrainOS usage')
  console.log('═'.repeat(60))

  const brain = new BrainOS()
  await brain.start()
  console.log('  🧠 BrainOS started')

  const agent = brain.agent('assistant')
  console.log('  🤖 Agent created')

  console.log('\n  ── Ask a question ──')
  console.log('  📤 User: "What is Python?"')

  try {
    const reply = await agent.ask('What is Python?')
    console.log(`  📥 Agent: ${reply.substring(0, 200)}`)
  } catch (e) {
    console.log(`  ⚠️  ${e.message}`)
    console.log('  ℹ️  Set OPENAI_API_KEY or create config.toml to use LLM')
  }

  await brain.stop()
  console.log('  🛑 BrainOS stopped')

  console.log('  ✅ Basic usage done\n')
}

async function demoCustomAgent() {
  console.log('═'.repeat(60))
  console.log('  Demo 2 — Custom agent config')
  console.log('═'.repeat(60))

  const apiKey = process.env.OPENAI_API_KEY
  if (!apiKey) {
    console.log('  ⚠️  OPENAI_API_KEY not set — skipping custom agent demo')
    console.log()
    return
  }

  const brain = new BrainOS({
    model: 'gpt-4',
    temperature: 0.5,
  })
  await brain.start()

  const agent = brain.agent('coder', {
    systemPrompt: 'You are a helpful coding assistant.',
  })

  console.log('  📤 User: "Hello"')
  const reply = await agent.ask('Hello')
  console.log(`  📥 Agent: ${reply.substring(0, 200)}`)

  await brain.stop()
  console.log('  ✅ Custom agent done\n')
}

async function main() {
  console.log(`\n  BrainOS JS — High-level Demo (v${version()})`)
  console.log('🧠'.repeat(30) + '\n')

  await demoBasicUsage()
  await demoCustomAgent()

  console.log('═'.repeat(60))
  console.log('  ✅ All BrainOS demos completed!')
  console.log('═'.repeat(60) + '\n')
}

main().catch(console.error)