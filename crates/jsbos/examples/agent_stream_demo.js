#!/usr/bin/env node
/**
 * Agent Streaming Demo
 *
 * Demonstrates the agent.stream() API for token-by-token streaming
 *
 * Usage: node agent_stream_demo.js
 */

import { Bus, Agent, ConfigLoader, initTracing } from '../index.js'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const config = JSON.parse(loader.loadSync())
const global = config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || global.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || global.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || global.model || 'nvidia/z-ai/glm4.7'

function addTool(args) {
  const a = parseFloat(args.a || 0)
  const b = parseFloat(args.b || 0)
  return JSON.stringify({ result: a + b })
}

const ADD_SCHEMA = {
  type: 'object',
  properties: {
    a: { type: 'number', description: 'First number' },
    b: { type: 'number', description: 'Second number' },
  },
  required: ['a', 'b'],
}

async function streamTask(agent, task) {
  console.log(`\n--- Stream: "${task}" ---`)

  await new Promise((resolve, reject) => {
    let tokenCount = 0
    let reasoningCount = 0

    agent.stream(task, (err, token) => {
      if (err) { reject(err); return }
      if (!token) return

      tokenCount++
      const type = token.type || 'unknown'

      switch (type) {
        case 'Text':
          if (token.text) process.stdout.write(token.text)
          break
        case 'ReasoningContent':
          reasoningCount++
          process.stderr.write('[thinking] ' + token.text)
          break
        case 'ToolCall':
          console.log('\n[ToolCall]', token.name, JSON.stringify(token.args))
          break
        case 'Done':
          console.log('\n[DONE] (' + tokenCount + ' tokens)')
          if (reasoningCount === 0) {
            console.log('[NO THINKING] Model did not emit reasoning_content chunks')
          }
          resolve()
          break
        case 'Error':
          console.log('\n[ERROR]', token.error)
          reject(new Error(token.error))
          break
      }
    })
  })
}

async function main() {
  console.log('\n=== BrainOS Streaming Demo ===\n')
  console.log('Model:', MODEL)

  const bus = await Bus.create()
  console.log('Bus created')

  if (!API_KEY) {
    console.log('No API key - set OPENAI_API_KEY or config.toml')
    process.exit(1)
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a helpful assistant.',
    temperature: 0.3,
    timeoutSecs: 60,
  }, bus)

  console.log('Agent created')

  await agent.addTool('add', 'Add numbers',
    JSON.stringify(ADD_SCHEMA.properties),
    JSON.stringify(ADD_SCHEMA),
    (_err, args) => addTool(args))
  console.log('Tools registered')

  await streamTask(agent, 'What is 2 + 3?')
  await streamTask(agent, 'What is 99 + 1?')

  console.log('\n=== Complete ===\n')
  process.exit(0)
}

main().catch(e => { console.error('Error:', e.message); process.exit(1) })