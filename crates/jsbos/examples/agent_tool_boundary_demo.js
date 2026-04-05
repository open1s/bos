#!/usr/bin/env node
/**
 * Agent Tool Call Boundary Verification Demo
 *
 * Verifies the strict boundary mechanism between tool calls and text:
 * 1. run_simple() — structured LlmResponse::ToolCall (AgentSession.run_loop)
 * 2. react() — text parsing with boundary rules (ReActEngine)
 *
 * Each mode is tested with:
 * - Tool mention in text (should NOT trigger tool execution)
 * - Explicit tool call (should execute)
 * - Mixed content (tool + text, should handle correctly)
 *
 * Usage:
 *     node crates/jsbos/examples/agent_tool_boundary_demo.js
 */

const { Bus, Agent, ConfigLoader, version } = require('../jsbos.js');

const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';

let toolCallsMade = [];

function calcCallback(args) {
  const a = parseFloat(args.a || 0);
  const b = parseFloat(args.b || 0);
  const op = args.op || 'add';
  let result;
  switch (op) {
    case 'add': result = a + b; break;
    case 'sub': result = a - b; break;
    case 'mul': result = a * b; break;
    case 'div': result = b !== 0 ? a / b : 'error'; break;
    default: result = 'unknown';
  }
  toolCallsMade.push({ tool: 'calc', args, result });
  return JSON.stringify({ result });
}

const CALC_SCHEMA = {
  type: 'object',
  properties: {
    a: { type: 'number' },
    b: { type: 'number' },
    op: { type: 'string', enum: ['add', 'sub', 'mul', 'div'] },
  },
  required: ['a', 'b', 'op'],
};

function greetCallback(args) {
  const name = args.name || 'World';
  toolCallsMade.push({ tool: 'greet', args, result: `Hello, ${name}!` });
  return JSON.stringify({ greeting: `Hello, ${name}!` });
}

const GREET_SCHEMA = {
  type: 'object',
  properties: {
    name: { type: 'string', description: "Person's name" },
  },
  required: ['name'],
};

async function createAgentWithTools(systemPrompt) {
  const bus = await Bus.create();
  const agent = await Agent.create({
    name: 'boundary-test',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: systemPrompt || (
      'You are a helpful assistant. ' +
      'Use the calc tool for math (op: add/sub/mul/div, a and b are numbers). ' +
      'Use the greet tool to greet someone by name. ' +
      'Always use tools when asked to calculate or greet. ' +
      'Final Answer: your response'
    ),
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus);
  await agent.addTool('calc', 'Math calculator', JSON.stringify(CALC_SCHEMA.properties), JSON.stringify(CALC_SCHEMA), (err, args) => calcCallback(args));
  await agent.addTool('greet', 'Greet someone', JSON.stringify(GREET_SCHEMA.properties), JSON.stringify(GREET_SCHEMA), (err, args) => greetCallback(args));
  return agent;
}

async function testRunSimple() {
  console.log('═'.repeat(60));
  console.log('  Test 1 — run_simple() (structured LlmResponse::ToolCall)');
  console.log('═'.repeat(60));

  toolCallsMade = [];
  const agent = await createAgentWithTools();

  const tests = [
    ['Tool mention in text', 'The calc tool can do math like calc(a=2,b=3,op=add). What is 5+3?'],
    ['Explicit tool request', 'Use the calc tool: a=10, b=3, op=mul'],
    ['Greet request', 'Greet BrainOS using the greet tool'],
  ];

  for (const [label, prompt] of tests) {
    toolCallsMade = [];
    console.log(`\n  [${label}] User: ${prompt}`);
    try {
      const reply = await agent.runSimple(prompt);
      console.log(`  [${label}] Agent: ${reply.substring(0, 300)}`);
      console.log(`  [${label}] Tool calls: ${JSON.stringify(toolCallsMade)}`);
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`);
    }
  }

  console.log('\n  ✅ run_simple() tests done\n');
}

async function testReact() {
  console.log('═'.repeat(60));
  console.log('  Test 2 — react() (ReActEngine with boundary mechanism)');
  console.log('═'.repeat(60));

  toolCallsMade = [];
  const agent = await createAgentWithTools(
    'You are a tool-calling assistant. ' +
    'When the user asks you to use a tool, call it with the appropriate arguments. ' +
    'After receiving the tool result, use it to provide your final answer. ' +
    "Do NOT repeat tool calls you've already seen the result for."
  );

  const tests = [
    ['Explicit tool request', 'Use the calc tool: a=7, b=8, op=mul'],
    ['Greet request', 'Greet BrainOS using the greet tool'],
  ];

  for (const [label, prompt] of tests) {
    toolCallsMade = [];
    console.log(`\n  [${label}] User: ${prompt}`);
    try {
      const reply = await agent.react(prompt);
      console.log(`  [${label}] Agent: ${reply.substring(0, 300)}`);
      console.log(`  [${label}] Tool calls: ${JSON.stringify(toolCallsMade)}`);
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`);
    }
  }

  console.log('\n  ✅ react() tests done\n');
}

async function main() {
  console.log('\n' + '🔬'.repeat(30));
  console.log('  BrainOS — Tool Call Boundary Verification');
  console.log('🔬'.repeat(30) + '\n');

  if (!API_KEY) {
    console.log('  ⚠️  OPENAI_API_KEY not set — LLM calls will fail');
    console.log('  Set: export OPENAI_API_KEY=sk-...\n');
    return;
  }

  await testRunSimple();
  await testReact();

  console.log('═'.repeat(60));
  console.log('  ✅ All boundary verification tests completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));
