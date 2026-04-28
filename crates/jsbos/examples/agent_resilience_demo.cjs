#!/usr/bin/env node
/**
 * Agent Resilience Configuration Demo
 *
 * Demonstrates:
 * 1. Circuit breaker configuration
 * 2. Rate limiter configuration
 * 3. Debug logging
 *
 * Usage:
 *     node examples/agent_resilience_demo.cjs                  # Basic
 *     RUST_LOG=debug node examples/agent_resilience_demo.cjs    # With Rust logs
 */

const { Bus, Agent, ConfigLoader, initTracing } = require('../index.js');

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
    return JSON.stringify({ error: 'Invalid characters' });
  }
  try {
    return JSON.stringify({ expression: expr, result: eval(expr) });
  } catch (e) {
    return JSON.stringify({ error: e.message });
  }
}

const CALCULATOR_SCHEMA = {
  type: 'object',
  properties: {
    expression: {
      type: 'string',
      description: "A math expression, e.g. '2 + 3 * 4'",
    },
  },
  required: ['expression'],
};

async function main() {
  console.log('\n=== BrainOS Resilience Configuration Demo ===\n');

  const bus = await Bus.create();
  console.log('[OK] Bus created');

  if (!API_KEY) {
    console.log('[SKIP] No API key — set OPENAI_API_KEY or ~/.bos/conf/config.toml');
    console.log('This demo requires an LLM API key.\n');
    return;
  }

  initTracing();
  console.log('[OK] Tracing initialized (RUST_LOG=debug for Rust logs)\n');

  console.log('--- DEMO 1: Default resilience ---');
  const agent1 = await Agent.create({
    name: 'demo1',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    temperature: 0.7,
    timeoutSecs: 120,
    systemPrompt: 'You are a math assistant.',
    rateLimitCapacity: 40,
    rateLimitWindowSecs: 60,
    rateLimitMaxRetries: 3,
    circuitBreakerMaxFailures: 5,
    circuitBreakerCooldownSecs: 30,
  }, bus);
  await agent1.addTool(
    'calculator',
    'Evaluate a math expression.',
    JSON.stringify(CALCULATOR_SCHEMA.properties),
    JSON.stringify(CALCULATOR_SCHEMA),
    (err, args) => calculatorTool(args),
  );
  console.log('[OK] Agent1: rateLimitCapacity=40, circuitBreakerMaxFailures=5');
  const r1 = await agent1.runSimple('What is 5 + 3?');
  console.log('  Result:', r1);

  console.log('\n--- DEMO 2: Strict resilience ---');
  const agent2 = await Agent.create({
    name: 'demo2',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    temperature: 0.7,
    timeoutSecs: 120,
    systemPrompt: 'You are a math assistant.',
    rateLimitCapacity: 2,
    rateLimitWindowSecs: 10,
    rateLimitMaxRetries: 1,
    circuitBreakerMaxFailures: 2,
    circuitBreakerCooldownSecs: 30,
  }, bus);
  await agent2.addTool(
    'calculator',
    'Evaluate a math expression.',
    JSON.stringify(CALCULATOR_SCHEMA.properties),
    JSON.stringify(CALCULATOR_SCHEMA),
    (err, args) => calculatorTool(args),
  );
  console.log('[OK] Agent2: rateLimitCapacity=2, circuitBreakerMaxFailures=2');
  const r2 = await agent2.runSimple('What is 10 + 20?');
  console.log('  Result:', r2);

  console.log('\n--- DEMO 3: Rate limiting only ---');
  const agent3 = await Agent.create({
    name: 'demo3',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    temperature: 0.7,
    timeoutSecs: 120,
    systemPrompt: 'You are a helpful assistant.',
    rateLimitCapacity: 100,
    rateLimitWindowSecs: 60,
    circuitBreakerMaxFailures: 1000,
  }, bus);
  await agent3.addTool(
    'calculator',
    'Evaluate a math expression.',
    JSON.stringify(CALCULATOR_SCHEMA.properties),
    JSON.stringify(CALCULATOR_SCHEMA),
    (err, args) => calculatorTool(args),
  );
  console.log('[OK] Agent3: rateLimitCapacity=100, circuitBreakerMaxFailures=1000');
  const r3 = await agent3.runSimple('What is 100 - 50?');
  console.log('  Result:', r3);

  console.log('\n=== All demos passed! ===');
  console.log('Run with RUST_LOG=debug to see Rust resilience logs.\n');
}

main().catch(console.error).finally(() => process.exit(0));