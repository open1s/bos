#!/usr/bin/env node
/**
 * Agent System Prompt Demo
 *
 * Demonstrates that systemPrompt actually reaches the LLM:
 * the same question is sent to two agents with different system
 * prompts, and the answers differ in tone/format as a result.
 *
 * Usage: node system_prompt_demo.js
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
  'You are a pirate captain. Answer every question in 1-2 short sentences ' +
  'in pirate speak, and always end your reply with "Arrr!".'

const CONCISE_PROMPT =
  'You are a terse assistant. Answer every question in EXACTLY three words. ' +
  'No more, no less.'

async function main() {
  console.log('\n=== BrainOS System Prompt Demo ===\n')
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

  console.log('\n[pirate agent]  systemPrompt:', JSON.stringify(PIRATE_PROMPT))
  console.log('[concise agent] systemPrompt:', JSON.stringify(CONCISE_PROMPT))
  console.log('\nQuestion:', question)

  const pirateAnswer = await pirate.runSimple(question)
  console.log('\n[pirate] ->', pirateAnswer)

  const conciseAnswer = await concise.runSimple(question)
  console.log('[concise] ->', conciseAnswer)

  const wordCount = conciseAnswer.trim().split(/\s+/).length
  console.log('\n[concise] word count:', wordCount, wordCount === 3 ? '(matches systemPrompt)' : '(DOES NOT match systemPrompt)')

  const endsWithArrr = /arrr!$/i.test(pirateAnswer.trim())
  console.log('[pirate]  ends with "Arrr!":', endsWithArrr ? 'yes' : 'no')

  process.exit(0)
}

main().catch(e => { console.error('Error:', e.message); process.exit(1) })
