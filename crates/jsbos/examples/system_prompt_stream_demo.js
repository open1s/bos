#!/usr/bin/env node
/**
 * Agent Streaming System Prompt Demo
 *
 * Demonstrates that systemPrompt reaches the LLM through the streaming
 * path (agent.stream) as well. The same question is streamed to two
 * agents with different system prompts, and the resulting tokens are
 * checked for the prompt-constrained patterns.
 *
 * Usage: node system_prompt_stream_demo.js
 */

import { Bus, Agent, ConfigLoader, initTracing } from '../index.js'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const config = JSON.parse(loader.loadSync())
const global = config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || global.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || global.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || global.model || 'nvidia/meta/llama-3.1-8b-instruct'

const PIRATE_PROMPT =
  'You are a pirate captain. Answer in 1-2 short sentences in pirate speak, ' +
  'and always end your reply with "Arrr!".'

const CONCISE_PROMPT =
  'You are a terse assistant. Answer every question in EXACTLY three words. ' +
  'No more, no less.'

function streamToString(agent, task) {
  return new Promise((resolve, reject) => {
    let text = ''
    agent.stream(task, (err, token) => {
      if (err) { reject(err); return }
      if (!token) return
      if (token.type === 'Text' && token.text) {
        process.stdout.write(token.text)
        text += token.text
      } else if (token.type === 'Error') {
        reject(new Error(token.error))
      } else if (token.type === 'Done') {
        resolve(text)
      }
    })
  })
}

async function main() {
  console.log('\n=== BrainOS Streaming System Prompt Demo ===\n')
  console.log('Model:', MODEL)

  const bus = await Bus.create()

  if (!API_KEY) {
    console.log('No API key - set OPENAI_API_KEY or config.toml')
    process.exit(1)
  }

  const pirate = await Agent.create({
    name: 'pirate',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: PIRATE_PROMPT,
    temperature: 0.7,
    timeoutSecs: 60,
  }, bus)

  const concise = await Agent.create({
    name: 'concise',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: CONCISE_PROMPT,
    temperature: 0.7,
    timeoutSecs: 60,
  }, bus)

  const question = 'What is the capital of France?'

  console.log('\n[pirate  agent] systemPrompt:', JSON.stringify(PIRATE_PROMPT))
  console.log('[concise agent] systemPrompt:', JSON.stringify(CONCISE_PROMPT))
  console.log('\nQuestion:', question)

  console.log('\n[pirate stream]')
  const pirateText = await streamToString(pirate, question)

  console.log('\n\n[concise stream]')
  const conciseText = await streamToString(concise, question)
  console.log('\n')

  const wordCount = conciseText.trim().split(/\s+/).filter(Boolean).length
  console.log('\n[concise] word count:', wordCount, wordCount === 3 ? '(matches systemPrompt)' : '(DOES NOT match systemPrompt)')

  const endsWithArrr = /arrr!$/i.test(pirateText.trim())
  console.log('[pirate]  ends with "Arrr!":', endsWithArrr ? 'yes' : 'no')

  process.exit(0)
}

main().catch(e => { console.error('Error:', e.message); process.exit(1) })
