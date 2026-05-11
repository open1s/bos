#!/usr/bin/env node
/**
 * Agent + MCP Tools Demo — LLM autonomously discovers and calls MCP tools
 *
 * Demonstrates:
 * 1. Creating an agent with LLM configuration
 * 2. Adding an MCP server — tools are auto-discovered and registered
 * 3. Running the agent — LLM decides when to call MCP tools vs answer directly
 * 4. Listing registered MCP tools
 *
 * Usage:
 *     node crates/jsbos/examples/agent_mcp_demo.js
 */

import { Bus, Agent, ConfigLoader, version, initTracing } from '../index.js'
import os from 'os'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const _config = JSON.parse(loader.loadSync())
const _global = _config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct'

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

async function demoMcpHelloWorldTools() {
  console.log('═'.repeat(60))
  console.log('  Demo 1 — Agent with MCP Hello World tools')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  const agent = await Agent.create({
    name: 'mcp-assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a tool-calling assistant. ' +
      'When asked to use a tool, output ONLY the tool call like: hello/echo(message="test")\n' +
      'After calling the tool, you will receive the result. ' +
      'Then provide your final answer based on the tool result.',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus)
  console.log('  🤖 Agent created')

  await agent.addMcpServer('hello', 'npx', ['-y', 'mcp-hello-world@latest'])
  console.log("  🔌 MCP server 'hello' connected")

  await sleep(500)

  const mcpTools = await agent.listMcpTools()
  console.log(`  🔧 MCP tools registered: ${mcpTools.length}`)
  for (const t of mcpTools) {
    console.log(`     - ${t.name}: ${(t.description || '').slice(0, 60)}`)
  }

  const allTools = agent.listTools()
  console.log(`  📋 Total tools available: ${allTools}`)

  const prompts = [
    ['Echo', 'Say hello to the world using the hello/echo tool'],
    ['Math', 'What is 3 plus 4? Use the add tool.'],
  ]

  for (const [label, prompt] of prompts) {
    console.log(`\n  [${label}] User: ${prompt}`)
    try {
      const reply = await agent.react(prompt)
      console.log(`  [${label}] Agent: ${reply.substring(0, 300)}`)
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`)
    }
  }

  console.log('\n  ✅ MCP Hello World demo done\n')
}

async function demoMcpFilesystemTools() {
  console.log('═'.repeat(60))
  console.log('  Demo 2 — Agent with MCP Filesystem tools')
  console.log('═'.repeat(60))

  const home = os.homedir()

  const bus = await Bus.create()
  const agent = await Agent.create({
    name: 'fs-assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a helpful assistant with filesystem access. ' +
      'Use the available tools to answer questions about files. ' +
      'Always show your reasoning before calling tools.',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus)
  console.log('  🤖 Agent created')

  await agent.addMcpServer('fs', 'npx', ['-y', '@modelcontextprotocol/server-filesystem@latest', home])
  console.log(`  🔌 MCP filesystem server connected (root: ${home})`)

  const mcpTools = await agent.listMcpTools()
  console.log(`  🔧 MCP tools registered: ${mcpTools.length}`)
  for (const t of mcpTools.slice(0, 5)) {
    console.log(`     - ${t.name}: ${(t.description || '').slice(0, 60)}`)
  }
  if (mcpTools.length > 5) {
    console.log(`     ... and ${mcpTools.length - 5} more`)
  }

  const prompts = [
    ['List dir', `List the contents of ${home} using the list_directory tool`],
  ]

  for (const [label, prompt] of prompts) {
    console.log(`\n  [${label}] User: ${prompt}`)
    try {
      const reply = await agent.react(prompt)
      console.log(`  [${label}] Agent: ${reply.substring(0, 300)}`)
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`)
    }
  }

  console.log('\n  ✅ MCP Filesystem demo done\n')
}

async function main() {
  console.log('\n' + '🧠'.repeat(30))
  console.log('  BrainOS — Agent + MCP Tools Demo')
  console.log('🧠'.repeat(30) + '\n')

  if (!API_KEY) {
    console.log('  ⚠️  OPENAI_API_KEY not set — demos will fail without a valid key')
    console.log('  Set: export OPENAI_API_KEY=sk-...\n')
  }

  await demoMcpHelloWorldTools()
  await demoMcpFilesystemTools()

  console.log('═'.repeat(60))
  console.log('  ✅ All Agent+MCP demos completed!')
  console.log('═'.repeat(60) + '\n')
}

main().catch(console.error).finally(() => process.exit(0))