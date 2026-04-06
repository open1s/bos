#!/usr/bin/env node
/**
 * Agent RPC Server Demo — Expose an agent as a callable server, call from another agent
 *
 * Demonstrates:
 * 1. Creating Agent A with MCP tools and exposing it as a callable server
 * 2. Creating Agent B that calls Agent A via RPC (tool/list, tool/call, llm/run)
 * 3. Agent-to-agent communication over the bus
 *
 * Architecture:
 *     Agent B (RPC client) ──bus──> Agent A (callable server + MCP tools)
 *                                        │
 *                                        └──> npx mcp-hello-world (echo, add, debug)
 *
 * Usage:
 *     export OPENAI_API_KEY="sk-..."
 *     export LLM_BASE_URL="https://integrate.api.nvidia.com/v1"
 *     export LLM_MODEL="nvidia/meta/llama-3.1-8b-instruct"
 *     node crates/jsbos/examples/agent_rpc_demo.js
 */

const { Bus, Agent, AgentConfig, ConfigLoader, version } = require('../jsbos.cjs');

const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';

async function demoAgentRpc() {
  console.log('═'.repeat(60));
  console.log('  Demo — Agent A as callable server, Agent B as RPC client');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  const session = await bus.session();
  console.log('  🔑 Bus session obtained');

  // ── Agent A: has MCP tools, exposed as callable server ──
  console.log('\n  ── Setting up Agent A (server side) ──');

  const configA = {
    name: 'agent-a',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a helpful assistant with math and greeting tools.',
    temperature: 0.7,
    timeoutSecs: 120,
  };
  const agentA = await Agent.createWithBus(configA, session);
  console.log('  🤖 Agent A created');

  await agentA.addMcpServer('hello', 'npx', ['-y', 'mcp-hello-world@latest']);
  console.log('  🔌 Agent A connected to MCP hello-world server');

  const mcpTools = await agentA.listMcpTools();
  console.log(`  🔧 Agent A MCP tools: ${mcpTools.map(t => t.name)}`);

  const server = await agentA.asCallableServer('agent-a-rpc', session);
  console.log(`  📡 Agent A callable server started`);
  console.log(`     endpoint: agent-a-rpc`);
  console.log(`     is_started: ${server.isStarted()}`);

  // ── Agent B: RPC client calling Agent A ──
  console.log('\n  ── Setting up Agent B (client side) ──');

  const configB = {
    name: 'agent-b',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a coordinator that calls other agents via RPC.',
    temperature: 0.7,
    timeoutSecs: 120,
  };
  const agentB = await Agent.createWithBus(configB, session);
  console.log('  🤖 Agent B created');

  const rpc = await agentB.rpcClient('agent-a-rpc', session);
  console.log(`  🔗 Agent B RPC client connected to: agent-a-rpc`);

  // ── RPC: tool/list ──
  console.log('\n  ── RPC: tool/list ──');
  const tools = await rpc.list();
  console.log(`  📋 Agent A's tools: ${JSON.stringify(tools)}`);

  // ── RPC: tool/call ──
  console.log('\n  ── RPC: tool/call ──');
  const result = await rpc.call('hello/add', JSON.stringify({ a: 7, b: 8 }));
  console.log('  📤 call(hello/add, {"a":7, "b":8})');
  console.log(`  📥 Result: ${JSON.stringify(result)}`);

  // ── RPC: llm/run ──
  console.log('\n  ── RPC: llm/run ──');
  console.log("  📤 llm_run('What is 3 + 5?')");
  try {
    const reply = await rpc.llmRun('What is 3 + 5? Use the hello/add tool.');
    const text = reply.text || String(reply);
    console.log(`  📥 Agent A reply: ${text.slice(0, 300)}`);
  } catch (e) {
    console.log(`  ⚠️  llm_run timed out or failed (LLM takes >10s over bus RPC): ${e}`);
    console.log('  ℹ️  Use tool/call for fast tool invocations; llm/run needs longer timeout');
  }

  console.log('\n  ✅ Agent RPC demo done\n');
}

async function main() {
  console.log('\n' + '🔗'.repeat(30));
  console.log('  BrainOS — Agent RPC Server Demo');
  console.log('🔗'.repeat(30) + '\n');

  if (!API_KEY) {
    console.log('  ⚠️  OPENAI_API_KEY not set — LLM calls will fail');
    console.log('  Set: export OPENAI_API_KEY=sk-...\n');
  }

  await demoAgentRpc();

  console.log('═'.repeat(60));
  console.log('  ✅ All Agent RPC demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main();