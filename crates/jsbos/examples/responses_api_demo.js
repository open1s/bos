#!/usr/bin/env node
/**
 * BrainOS — DeepSeek Responses API Demo
 *
 * Demonstrates the OpenAI Responses API (`/v1/responses`) end-to-end:
 *   1. run_simple()   — non-stream, plain Q&A
 *   2. react()        — function_call round-trip with a registered tool
 *   3. stream()       — streaming SSE (callback form)
 *   4. streamCollect()— streaming SSE (collect-ahead form)
 *
 * Uses `api_mode = "responses"` + `reasoning_effort = "high"`. The reasoning
 * `usage` tokens surface in the stream output.
 *
 * Usage:
 *     node crates/jsbos/examples/responses_api_demo.js
 *
 * Credentials (env overrides config):
 *     OPENAI_API_KEY / LLM_BASE_URL / LLM_MODEL
 *     e.g. LLM_BASE_URL=https://api.deepseek.com LLM_MODEL=deepseek-v4-flash
 */

import { Agent, ConfigLoader, initTracing } from '../index.js'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const _config = JSON.parse(loader.loadSync())
const _global = _config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || ''
const BASE_URL =
  process.env.LLM_BASE_URL || _global.base_url || 'https://api.deepseek.com'
const MODEL = process.env.LLM_MODEL || _global.model || 'deepseek-v4-flash'

function addTool(args) {
  const a = Number(args.a) || 0
  const b = Number(args.b) || 0
  return JSON.stringify({ sum: a + b })
}

const ADD_SCHEMA = {
  type: 'object',
  properties: {
    a: { type: 'number', description: 'First integer' },
    b: { type: 'number', description: 'Second integer' },
  },
  required: ['a', 'b'],
}

// Collect-ahead: gather all streamed tokens until the terminal event.
// (The native Agent has `stream`, not `streamCollect`; wrap it here.)
async function collectTokens(agent, task) {
  return new Promise((resolve, reject) => {
    const tokens = []
    agent.stream(task, (err, token) => {
      if (err) {
        reject(typeof err === 'string' ? new Error(err) : err)
        return
      }
      if (!token) return
      tokens.push(token)
      if (token.type === 'Done' || token.type === 'Error' || token.type === 'Stopped') {
        if (token.type === 'Error') reject(new Error(token.error))
        else resolve(tokens)
      }
    })
  })
}

async function main() {
  console.log('\n' + '◆'.repeat(28))
  console.log('  BrainOS — Responses API Demo')
  console.log('◆'.repeat(28))

  if (!API_KEY) {
    console.log(
      '  ⚠️  No API key — set OPENAI_API_KEY or add [global_model] to ~/.bos/conf/config.toml',
    )
    console.log('  Skipping demo\n')
    return
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a helpful assistant. Use the add tool to perform integer addition.',
    temperature: 0.7,
    timeoutSecs: 120,
    apiMode: 'responses',
    reasoningEffort: 'high',
  })

  console.log(`  Agent online: ${MODEL} @ ${BASE_URL}`)
  console.log(`  api_mode=responses  reasoning_effort=high`)

  console.log('\n' + '═'.repeat(56))
  console.log('  1. run_simple() — non-stream Q&A')
  console.log('═'.repeat(56))
  try {
    const reply = await agent.runSimple('Say hi in one word')
    console.log(`  Response: ${reply}`)
  } catch (e) {
    console.log(`  ⚠️  ${e.message}`)
  }

  await agent.addTool(
    'add',
    'Add two integers and return their sum.',
    JSON.stringify(ADD_SCHEMA.properties),
    JSON.stringify(ADD_SCHEMA),
    (err, args) => addTool(args),
    false,
  )
  console.log('  ✅ Registered tool: add')

  console.log('\n' + '═'.repeat(56))
  console.log('  2. react() — function_call round-trip')
  console.log('═'.repeat(56))
  try {
    const reply = await agent.react('What is 3 + 4? Use the add tool.')
    console.log(`  Response: ${reply}`)
  } catch (e) {
    console.log(`  ⚠️  ${e.message}`)
  }

  console.log('\n' + '═'.repeat(56))
  console.log('  3. stream() — SSE (callback form)')
  console.log('═'.repeat(56))
  try {
    let text = ''
    await agent.stream('Count from 1 to 3, one per line', (err, token) => {
      if (err) return
      if (!token) return
      if (token.type === 'Text') {
        text += token.text
        process.stdout.write(token.text)
      } else if (token.type === 'ReasoningContent') {
        process.stdout.write(`[⟳${token.text}]`)
      } else if (token.type === 'Usage') {
        console.error(
          `\n  [usage] prompt=${token.promptTokens} completion=${token.completionTokens} total=${token.totalTokens}`,
        )
      }
    })
  } catch (e) {
    console.log(`  ⚠️  ${e.message}`)
  }

  console.log('\n' + '═'.repeat(56))
  console.log('  4. collect-ahead stream (SSE)')
  console.log('═'.repeat(56))
  try {
    const tokens = await collectTokens(agent, 'What is 5 + 3? Use the add tool.')
    const types = tokens.map((t) => t.type).join(', ')
    console.log(`  Collected ${tokens.length} tokens: [${types}]`)
    const text = tokens
      .filter((t) => t.type === 'Text')
      .map((t) => t.text)
      .join('')
    console.log(`  Text: ${text}`)
  } catch (e) {
    console.log(`  ⚠️  ${e.message}`)
  }

  console.log('\n' + '═'.repeat(56))
  console.log('  ✅ Responses API demo completed!')
  console.log('═'.repeat(56) + '\n')
}

main().catch(console.error).finally(() => process.exit(0))