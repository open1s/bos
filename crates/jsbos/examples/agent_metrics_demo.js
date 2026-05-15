#!/usr/bin/env node
/**
 * Agent Performance Metrics Demo
 *
 * Calls react(), run_simple(), and stream() then shows getPerfMetrics()
 * to verify token usage and call counts are recorded correctly.
 *
 * Usage: node agent_metrics_demo.js
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

function printMetrics(label, metrics) {
  console.log(`\n--- ${label} ---`)
  console.log(`  llmCallCount:         ${metrics.llmCallCount}`)
  console.log(`  llmErrors:            ${metrics.llmErrors}`)
  console.log(`  totalWallTimeMs:      ${(metrics.totalWallTimeUs / 1000).toFixed(1)}`)
  console.log(`  totalEngineTimeMs:    ${(metrics.totalEngineTimeUs / 1000).toFixed(1)}`)
  console.log(`  avgWallTimeMs:        ${(metrics.avgWallTimeUs / 1000).toFixed(1)}`)
  console.log(`  totalInputTokens:     ${metrics.totalInputTokens}`)
  console.log(`  totalOutputTokens:    ${metrics.totalOutputTokens}`)
  console.log(`  totalTokens:          ${metrics.totalInputTokens + metrics.totalOutputTokens}`)
  console.log(`  toolInvocationCount:  ${metrics.toolInvocationCount}`)
}

async function main() {
  console.log('\n=== BrainOS Performance Metrics Demo ===\n')
  console.log('Model:', MODEL)

  if (!API_KEY) {
    console.log('No API key - set OPENAI_API_KEY or config.toml')
    process.exit(1)
  }

  const bus = await Bus.create()

  const agent = await Agent.create({
    name: 'metrics-demo',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a helpful assistant. Keep answers brief.',
    temperature: 0.3,
    timeoutSecs: 60,
  }, bus)

  // Initial metrics - all zeros
  printMetrics('Initial (before any calls)', agent.getPerfMetrics())

  // 1. Call react()
  console.log('\n>>> Calling agent.react("Say hello briefly") ...')
  const reactResult = await agent.react('Say hello briefly')
  console.log('React result:', reactResult.substring(0, 100))
  printMetrics('After react()', agent.getPerfMetrics())

  // 2. Call run_simple()
  console.log('\n>>> Calling agent.runSimple("What is 2+2?") ...')
  const simpleResult = await agent.runSimple('What is 2+2?')
  console.log('RunSimple result:', simpleResult.substring(0, 100))
  printMetrics('After runSimple()', agent.getPerfMetrics())

  // 3. Call stream()
  console.log('\n>>> Calling agent.stream("List 3 colors") ...')
  await new Promise((resolve, reject) => {
    agent.stream('List 3 colors', (err, token) => {
      if (err) { reject(err); return }
      if (!token) return
      if (token.type === 'Text' && token.text) process.stdout.write(token.text)
      if (token.type === 'Done') { console.log(); resolve() }
      if (token.type === 'Error') { console.log('\n[ERROR]', token.error); reject(new Error(token.error)) }
    })
  })
  printMetrics('After stream()', agent.getPerfMetrics())

  // Summary
  const final = agent.getPerfMetrics()
  console.log('\n=== Summary ===')
  console.log(`Total LLM calls:         ${final.llmCallCount}`)
  console.log(`Total LLM errors:        ${final.llmErrors}`)
  console.log(`Total input tokens:      ${final.totalInputTokens}`)
  console.log(`Total output tokens:     ${final.totalOutputTokens}`)
  console.log(`Total tokens:            ${final.totalInputTokens + final.totalOutputTokens}`)
  console.log(`Total tool invocations:  ${final.toolInvocationCount}`)
  console.log(`Total wall time:         ${(final.totalWallTimeUs / 1000).toFixed(0)}ms`)

  if (final.llmCallCount === 3) {
    console.log('\n✓ All 3 LLM calls recorded correctly!')
  } else {
    console.log(`\n✗ Expected 3 LLM calls, got ${final.llmCallCount}`)
  }
  if (final.totalInputTokens > 0 || final.totalOutputTokens > 0) {
    console.log('✓ Token usage recorded!')
  } else {
    console.log('✗ Token usage NOT recorded (LLM may not return usage info)')
  }

  console.log('\n=== Complete ===\n')
  process.exit(0)
}

main().catch(e => { console.error('Error:', e.message); process.exit(1) })
