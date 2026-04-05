#!/usr/bin/env node
/**
 * Agent Tool Calling & LLM Conversation Demo
 *
 * Demonstrates:
 * 1. Registering JS tools (calculator, weather, time)
 * 2. LLM autonomously deciding when to call tools vs answer directly
 * 3. Multi-turn conversation with tool results feeding back into reasoning
 *
 * Usage:
 *     node crates/jsbos/examples/agent_demo.js
 */

const { Bus, Agent, ConfigLoader, version } = require('../jsbos.js');

const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';

function calculatorTool(args) {
  const expr = args.expression || '';
  const allowed = new Set('0123456789+-*/.() ');
  if (![...expr].every(c => allowed.has(c))) {
    return JSON.stringify({ error: 'Invalid characters in expression' });
  }
  try {
    const result = eval(expr);
    return JSON.stringify({ expression: expr, result });
  } catch (e) {
    return JSON.stringify({ error: e.message });
  }
}

const CALCULATOR_SCHEMA = {
  type: 'object',
  properties: {
    expression: {
      type: 'string',
      description: "A math expression to evaluate, e.g. '2 + 3 * 4'",
    },
  },
  required: ['expression'],
};

function weatherTool(args) {
  const city = args.city || 'unknown';
  const mockData = {
    city,
    temperature: 22,
    unit: '°C',
    condition: 'sunny',
    humidity: 45,
  };
  return JSON.stringify(mockData);
}

const WEATHER_SCHEMA = {
  type: 'object',
  properties: {
    city: {
      type: 'string',
      description: "City name, e.g. 'Beijing', 'San Francisco'",
    },
  },
  required: ['city'],
};

function timeTool() {
  const now = new Date().toISOString();
  return JSON.stringify({ utc_time: now, timezone: 'UTC' });
}

const TIME_SCHEMA = {
  type: 'object',
  properties: {},
};

async function chatWithTools(agent, userInput) {
  return await agent.runSimple(userInput);
}

async function main() {
  console.log('\n' + '🧠'.repeat(30));
  console.log('  BrainOS — Agent Tool Calling & Conversation Demo');
  console.log('🧠'.repeat(30));

  const bus = await Bus.create();
  console.log('  🚌 Bus created');

  if (!API_KEY) {
    console.log('  ⚠️  No API key found — set OPENAI_API_KEY or create ~/.bos/conf/config.toml');
    console.log('  Skipping LLM demo\n');
    return;
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a helpful assistant. ' +
      'Use the available tools when they can help answer the question. ' +
      'Format: Thought: <reasoning>\nFinal Answer: <response or tool call>',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus);

  console.log('\n' + '═'.repeat(60));
  console.log('  Step 1 — Registering Tools');
  console.log('═'.repeat(60));

  await agent.addTool(
    'calculator',
    'Evaluate a mathematical expression and return the result.',
    JSON.stringify(CALCULATOR_SCHEMA.properties),
    JSON.stringify(CALCULATOR_SCHEMA),
    (err, args) => calculatorTool(args),
  );
  console.log('  ✅ Registered tool: calculator');

  await agent.addTool(
    'weather',
    'Get current weather information for a given city.',
    JSON.stringify(WEATHER_SCHEMA.properties),
    JSON.stringify(WEATHER_SCHEMA),
    (err, args) => weatherTool(args),
  );
  console.log('  ✅ Registered tool: weather');

  await agent.addTool(
    'current_time',
    'Get the current UTC time.',
    JSON.stringify(TIME_SCHEMA.properties),
    JSON.stringify(TIME_SCHEMA),
    (err, args) => timeTool(args),
  );
  console.log('  ✅ Registered tool: current_time');

  console.log(`\n  Available tools: ${agent.listTools()}`);

  console.log('\n' + '═'.repeat(60));
  console.log('  Step 2 — Agent Tool Calling (LLM decides when to use tools)');
  console.log('═'.repeat(60));

  const prompts = [
    ['Math', 'What is 1234 * 5678?'],
    ['Weather', "What's the weather like in Tokyo right now?"],
    ['Time', 'What time is it now in UTC?'],
    ['Mixed', 'Calculate 99 * 99 and tell me the weather in Paris.'],
  ];

  for (const [label, prompt] of prompts) {
    console.log(`\n  [${label}] User: ${prompt}`);
    try {
      const reply = await chatWithTools(agent, prompt);
      console.log(`  [${label}] Agent: ${reply}`);
    } catch (e) {
      console.log(`  [${label}] ⚠️  ${e.message}`);
    }
  }

  console.log('\n' + '═'.repeat(60));
  console.log('  ✅ Demo completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));
