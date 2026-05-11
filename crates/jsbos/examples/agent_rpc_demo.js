#!/usr/bin/env node
/**
 * Agent RPC Server Demo — Expose an agent as a callable server, call from another agent
 *
 * Note: This demo shows MCP tool integration. Full RPC server functionality
 * requires agent creation with bus session (not yet exposed in JS bindings).
 *
 * Demonstrates:
 * 1. Creating Agent with MCP tools
 * 2. Listing and using MCP tools via agent
 *
 * Usage:
 * export OPENAI_API_KEY="sk-..."
 * export LLM_BASE_URL="https://integrate.api.nvidia.com/v1"
 * export LLM_MODEL="nvidia/meta/llama-3.1-8b-instruct"
 * node crates/jsbos/examples/agent_rpc_demo.js
 */

import { Bus, Agent, ConfigLoader, version, initTracing } from '../index.js'

initTracing()

const loader = new ConfigLoader()
loader.discover()
const _config = JSON.parse(loader.loadSync())
const _global = _config.global_model || {}

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || ''
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1'
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct'

async function demoAgentMcp() {
  console.log('═'.repeat(60))
  console.log(' Demo — Agent with MCP tools')
  console.log('═'.repeat(60))

  const bus = await Bus.create()
  console.log(' 🚌 Bus created')

  console.log('\n ── Setting up Agent with MCP tools ──')

  const configA = {
    name: 'agent-with-mcp',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a helpful assistant with math and greeting tools.',
    temperature: 0.7,
    timeoutSecs: 120,
  }
  const agent = await Agent.create(configA, bus)
  console.log(' 🤖 Agent created')

  await agent.addMcpServer('hello', 'npx', ['-y', 'mcp-hello-world@latest'])
  console.log(' 🔌 Agent connected to MCP hello-world server')

  await new Promise(resolve => setTimeout(resolve, 2000))

  const mcpTools = await agent.listMcpTools()
  console.log(` 🔧 MCP tools available: ${mcpTools.map(t => t.name).join(', ')}`)

  console.log('\n ── Test: Agent react() with MCP tool ──')
  console.log(" 📤 User: 'Use the hello/add tool to calculate 7 + 8'")
  try {
    const result = await agent.react('Use the hello/add tool to calculate 7 + 8')
    console.log(` 📥 Agent: ${result.slice(0, 200)}...`)
  } catch (e) {
    console.log(` ⚠️ React failed: ${e.message}`)
  }

  console.log('\n ✅ Agent MCP demo done\n')
}

async function main() {
  console.log('\n' + '🔗'.repeat(30))
  console.log(' BrainOS — Agent MCP Demo')
  console.log('🔗'.repeat(30) + '\n')

  if (!API_KEY) {
    console.log(' ⚠️ OPENAI_API_KEY not set — LLM calls will fail')
    console.log(' Set: export OPENAI_API_KEY=sk-...\n')
  }

  await demoAgentMcp()

  console.log('═'.repeat(60))
  console.log(' ✅ All Agent demos completed!')
  console.log('═'.repeat(60) + '\n')
}

main()