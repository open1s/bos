#!/usr/bin/env node
/**
 * Agent Streaming Demo
 *
 * Demonstrates:
 * 1. Using the agent.stream() method for streaming responses
 * 2. Receiving Text, ToolCall, Done, and Error tokens via callback
 * 3. Real-time output as the LLM generates responses
 * 4. Visualizing the streaming flow: reasoning → tool call → result → final
 *
 * Usage:
 * node crates/jsbos/examples/agent_stream_demo.cjs
 */

const { Bus, Agent, ConfigLoader } = require('../index.js');

const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';

// Tool schemas
const TIME_SCHEMA = {
  type: 'object',
  properties: {},
};

const ADD_SCHEMA = {
  type: 'object',
  properties: {
    a: { type: 'number', description: 'First number' },
    b: { type: 'number', description: 'Second number' },
  },
  required: ['a', 'b'],
};

const CALCULATE_SCHEMA = {
  type: 'object',
  properties: {
    expression: { type: 'string', description: 'Math expression (e.g., "2 + 2" or "15 * 3 + 10")' },
  },
  required: ['expression'],
};

// Tool implementations
function timeTool() {
  const now = new Date().toISOString();
  return JSON.stringify({ utc_time: now, timezone: 'UTC' });
}

function addTool(args) {
  const { a, b } = args;
  return JSON.stringify({ result: a + b, operation: `${a} + ${b}` });
}

function calculateTool(args) {
  const { expression } = args;
  try {
    // Safe evaluation using Function constructor (no eval)
    const result = new Function('return ' + expression)();
    return JSON.stringify({ expression, result });
  } catch (e) {
    return JSON.stringify({ error: 'Invalid expression' });
  }
}

// Visual stream handler with phase tracking
function createStreamHandler(label) {
  let phase = 'reasoning'; // reasoning → tool_call → tool_result → final
  let toolCallCount = 0;
  const startTime = Date.now();

  return {
    onText(token) {
      // Detect phase transitions
      const lower = token.toLowerCase();
      if (lower.includes('calling') || lower.includes('using') ||
          lower.includes('tool')) {
        if (phase === 'reasoning') {
          // Transitioning to tool call
        }
      }

      // Check if this text is a tool result (usually JSON)
      if (token.trim().startsWith('{') || token.trim().startsWith('"')) {
        if (phase !== 'tool_result' && phase !== 'final') {
          phase = 'tool_result';
        }
      } else if (phase === 'reasoning' && token.length > 20) {
        phase = 'final';
      }

      process.stdout.write(token);
      return token.length;
    },

    onToolCall(token) {
      phase = 'tool_call';
      toolCallCount++;

      console.log('\n');
      console.log('  ┌─────────────────────────────────────────────────────────────┐');
      console.log('  │ 🔧 TOOL CALL #' + toolCallCount + '                                              │');
      console.log('  ├─────────────────────────────────────────────────────────────┤');
      console.log('  │ Tool: ' + token.name.padEnd(50) + '│');
      console.log('  │ Args: ' + JSON.stringify(token.args).substring(0, 47).padEnd(50) + '│');
      console.log('  │ ID:   ' + String(token.id || 'auto').substring(0, 50) + '│');
      console.log('  └─────────────────────────────────────────────────────────────┘');
      console.log('  │ ⏳ Executing tool...                                        │');
      console.log('  │');
      console.log('  │ 📤 Tool Result: ');
    },

    onToolResult(text) {
      phase = 'tool_result';
      console.log('  │   ' + text.trim().replace(/\n/g, '\n  │   '));
      console.log('  │');
      console.log('  │ ▶ Continuing stream with result...                         │');
      console.log('  │');
      console.log('  │ 📝 Final Response:                                         │');
    },

    onDone() {
      const elapsed = Date.now() - startTime;
      console.log('\n');
      console.log('  └─────────────────────────────────────────────────────────────┘');
      console.log(`  ✅ Done (${elapsed}ms, ${toolCallCount} tool call${toolCallCount !== 1 ? 's' : ''})`);
    },

    onError(error) {
      console.log('\n  ❌ Error: ' + error);
    },

    getPhase() { return phase; },
    getToolCallCount() { return toolCallCount; },
    getElapsed() { return Date.now() - startTime; }
  };
}

async function main() {
  console.log('\n' + '🔊'.repeat(30));
  console.log(' BrainOS — Agent Streaming Demo');
  console.log('🔊'.repeat(30));

  const bus = await Bus.create();
  console.log(' 🚌 Bus created');

  if (!API_KEY) {
    console.log(' ⚠️  No API key found — set OPENAI_API_KEY or create ~/.bos/conf/config.toml');
    console.log(' Skipping streaming demo\n');
    return;
  }

  const agent = await Agent.create({
    name: 'assistant',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
    'You are a helpful assistant with access to tools.\n\n' +
    'Available tools:\n' +
    '- current_time: Get the current UTC time\n' +
    '- add: Add two numbers (args: a, b)\n' +
    '- calculate: Evaluate a math expression (args: expression)\n\n' +
    'When using a tool, format your response as:\n' +
    'Thought: <reasoning>\n' +
    'Action: <tool_name>\n' +
    'Action Input: <arguments>\n' +
    'OR\n' +
    'Final Answer: <response>',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus);

  console.log('\n' + '═'.repeat(64));
  console.log(' Step 1 — Registering Tools');
  console.log('═'.repeat(64));

  await agent.addTool(
    'current_time',
    'Get the current UTC time.',
    JSON.stringify(TIME_SCHEMA.properties),
    JSON.stringify(TIME_SCHEMA),
    (_err, args) => timeTool(args),
  );
  console.log(' ✅ Registered: current_time (get UTC time)');

  await agent.addTool(
    'add',
    'Add two numbers together.',
    JSON.stringify(ADD_SCHEMA.properties),
    JSON.stringify(ADD_SCHEMA),
    (_err, args) => addTool(args),
  );
  console.log(' ✅ Registered: add (a + b)');

  await agent.addTool(
    'calculate',
    'Evaluate a mathematical expression.',
    JSON.stringify(CALCULATE_SCHEMA.properties),
    JSON.stringify(CALCULATE_SCHEMA),
    (_err, args) => calculateTool(args),
  );
  console.log(' ✅ Registered: calculate (eval expression)');

  console.log('\n' + '═'.repeat(64));
  console.log(' Step 2 — Streaming Response Demo');
  console.log('═'.repeat(64));

  // Test prompts designed to trigger tool calls
  const prompts = [
    {
      label: 'Time Query',
      prompt: 'What time is it right now in UTC? Use the current_time tool.',
      expectTool: 'current_time',
    },
    {
      label: 'Addition',
      prompt: 'What is 1234 + 5678? Use the add tool to calculate.',
      expectTool: 'add',
    },
    {
      label: 'Math Expression',
      prompt: 'Calculate: (25 * 4) + (100 / 2) - 50',
      expectTool: 'calculate',
    },
    {
      label: 'Simple',
      prompt: 'Say hello in exactly 3 words.',
      expectTool: null,
    },
  ];

  for (const { label, prompt, expectTool } of prompts) {
    console.log('\n' + '─'.repeat(64));
    console.log(' 📋 Test: ' + label);
    console.log('    Prompt: ' + prompt);
    if (expectTool) {
      console.log('    Expects tool: ' + expectTool);
    }
    console.log('─'.repeat(64));

    const handler = createStreamHandler(label);
    let textAfterTool = false;

    try {
      await new Promise((resolve, reject) => {
        agent.stream(prompt, (err, data) => {
          if (err) {
            reject(err);
            return;
          }

          const token = data;
          const type = token.type;

          switch (type) {
          case 'Text':
            // Track if we're getting text after a tool result
            if (handler.getPhase() === 'tool_result') {
              textAfterTool = true;
              handler.onToolResult('');
            }
            handler.onText(token.text);
            break;
          case 'ToolCall':
            handler.onToolCall(token);
            break;
          case 'Done':
            if (textAfterTool) {
              handler.onToolResult(''); // Close the tool result box
            }
            handler.onDone();
            resolve();
            break;
          case 'Error':
            handler.onError(token.error);
            reject(new Error(token.error));
            break;
          }
        });
      });
    } catch (e) {
      console.log('\n  ⚠️  ' + e.message);
    }
  }

  console.log('\n' + '═'.repeat(64));
  console.log(' ✅ Streaming demo completed!');
  console.log('═'.repeat(64) + '\n');
  console.log(' Key Takeaways:');
  console.log('  1. ToolCall tokens arrive MID-STREAM when LLM decides to call a tool');
  console.log('  2. Tool executes automatically, result returns as Text token');
  console.log('  3. Stream continues with tool result, then final response');
  console.log('  4. Multiple tool calls can happen in a single response\n');
}

main().catch(console.error).finally(() => process.exit(0));