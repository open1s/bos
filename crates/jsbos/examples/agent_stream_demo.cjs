#!/usr/bin/env node
/**
 * Agent Streaming Demo
 * 
 * Demonstrates the agent.stream() API and token types
 * 
 * Usage: node agent_stream_demo.cjs
 */

const { Bus, Agent, ConfigLoader,initTracing } = require('../index.js');

initTracing();

const loader = new ConfigLoader();
loader.discover();
const config = JSON.parse(loader.loadSync());
const global = config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || global.model || 'nvidia/nvidia/nemotron-mini-4b-instruct';

function addTool(args) {
  return JSON.stringify({ result: args.a + args.b + "hello" });
}

function echoTool(args) {
  return JSON.stringify({ reversed: args.text.split('').reverse().join('') });
}

async function main() {
  console.log('\n=== BrainOS Streaming Demo ===\n');
  console.log('Model:', MODEL);

  const bus = await Bus.create();
  console.log('Bus created');

  if (!API_KEY) {
    console.log('No API key - set OPENAI_API_KEY or config.toml');
    process.exit(1);
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'You are a helpful assistant.',
    temperature: 0.3,
    timeoutSecs: 60,
  }, bus);

  console.log('Agent created');

  await agent.addTool('add', 'Add numbers',
    JSON.stringify({ a: { type: 'number' }, b: { type: 'number' }}),
    JSON.stringify({ a: { type: 'number' }, b: { type: 'number' }, required: ['a', 'b'] }),
    (_err, args) => addTool(args));

  await agent.addTool('echo', 'Echo text',
    JSON.stringify({ text: { type: 'string' }}),
    JSON.stringify({ text: { type: 'string' }, required: ['text'] }),
    (_err, args) => echoTool(args));
  console.log('Tool: echo');

  console.log('\n--- Stream: "What is 2 + 3?" ---');

  await new Promise((resolve, reject) => {
    let tokenCount = 0;
    agent.stream('What is 2 + 3?', (err, token) => {
      if (err) { reject(err); return; }
      if (!token) return;
      
      tokenCount++;
      const type = token.type || 'unknown';
      
      switch (type) {
        case 'Text':
          if (token.text) process.stdout.write(token.text);
          break;
        case 'ToolCall':
          console.log('\nTool:', token.name, JSON.stringify(token.args));
          break;
        case 'Done':
          console.log('\nDone (' + tokenCount + ' tokens)');
          resolve();
          break;
        case 'Error':
          console.log('\nError:', token.error);
          reject(new Error(token.error));
          break;
      }
    });
  });

  console.log('\n--- Stream: "Echo hello" ---');

  await new Promise((resolve, reject) => {
    let tokenCount = 0;
    agent.stream('Echo "hello"', (err, token) => {
      if (err) { reject(err); return; }
      if (!token) return;
      
      tokenCount++;
      const type = token.type || 'unknown';
      
      switch (type) {
        case 'Text':
          if (token.text) process.stdout.write(token.text);
          break;
        case 'ToolCall':
          console.log('\nTool:', token.name, JSON.stringify(token.args));
          break;
        case 'Done':
          console.log('\nDone (' + tokenCount + ' tokens)');
          resolve();
          break;
        case 'Error':
          console.log('\nError:', token.error);
          reject(new Error(token.error));
          break;
      }
    });
  });

  console.log('\n=== Complete ===\n');
}

main().catch(e => { console.error('Error:', e.message); process.exit(1); });
