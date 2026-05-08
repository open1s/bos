const { BrainOS, Config, enableTracing, ToolDef } = require('../brainos.js');
const raw = require('../index.js');
const fs = require('fs');
const path = require('path');

// enableTracing(); // Enable detailed logging for tracing the elegant API features

async function main() {
  console.log('═══════════════════════════════════════════════════════════');
  console.log('           BrainOS - Tool/Skill/Plugin/Hook Demo');
  console.log('════════════════════════════════════════════════════════════\n');

  const config = Config.load();
  console.log('Config:', config, '\n');

  const brain = new BrainOS();
  await brain.start();

  // Create tools
  const addTool = new ToolDef(
    'add',
    'Add two numbers',
    (args) => (args.a || 0) + (args.b || 0),
    { type: 'object', properties: { result: { type: 'number' } }, required: ['result'] },
    { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] }
  );

  const multiplyTool = new ToolDef(
    'multiply',
    'Multiply two numbers',
    (args) => (args.a || 1) * (args.b || 1),
    { type: 'object', properties: { result: { type: 'number' } }, required: ['result'] },
    { type: 'object', properties: { a: { type: 'number' }, b: { type: 'number' } }, required: ['a', 'b'] }
  );

  const uppercaseTool = new ToolDef(
    'uppercase',
    'Convert to uppercase',
    (args) => String(args.text || '').toUpperCase(),
    { type: 'object', properties: { result: { type: 'string' } }, required: ['result'] },
    { type: 'object', properties: { text: { type: 'string' } }, required: ['text'] }
  );

  // Create skills dir
  const skillsDir = path.join(__dirname, '.demo-skills');
  if (!fs.existsSync(skillsDir)) fs.mkdirSync(skillsDir, { recursive: true });
  const skillDir = path.join(skillsDir, 'python-coding');
  if (!fs.existsSync(skillDir)) fs.mkdirSync(skillDir, { recursive: true });

  fs.writeFileSync(path.join(skillDir, 'SKILL.md'), `---
name: python-coding
description: Python coding conventions
category: coding
version: 1.0.0
---

# Python Coding

Use type hints, snake_case, docstrings.`);

  const agent = await brain
    .agent('demo-agent')
    .system('You are a helpful assistant with tools. Use them when needed.')
    .model(config.model)
    .register(addTool)
    .register(multiplyTool)
    .register(uppercaseTool)
    .plugin('DemoPlugin', {
      onLlmRequest: (req) => { console.log('  [Plugin:on_llm_request]'); return req; },
      onLlmResponse: (resp) => { console.log('  [Plugin:on_llm_response]'); return resp; },
      onToolCall: (call) => { console.log('  [Plugin:on_tool_call]', call.name); return call; },
      onToolResult: (result) => { console.log('  [Plugin:on_tool_result]'); return result; }
    })
    .hook(raw.HookEvent.BeforeToolCall, (err, ctx) => { if (err) return 'error'; console.log('  [Hook:BeforeToolCall]', ctx?.data?.tool_name || 'unknown'); return 'continue'; })
    .hook(raw.HookEvent.AfterToolCall, (err, ctx) => { if (err) return 'error'; console.log('  [Hook:AfterToolCall]', ctx?.data?.tool_name || 'unknown'); return 'continue'; })
    .hook(raw.HookEvent.BeforeLlmCall, (err, ctx) => { if (err) return 'error'; console.log('  [Hook:BeforeLlmCall]'); return 'continue'; })
    .hook(raw.HookEvent.AfterLlmCall, (err, ctx) => { if (err) return 'error'; console.log('  [Hook:AfterLlmCall]'); return 'continue'; })
    .skillsFromDir(skillsDir)
    .start();

  console.log('───────────────────────────────────────────────────────────');
  console.log('1. TOOLS - Add tools with proper schema');
  console.log('───────────────────────────────────────────────────────────\n');

  console.log('Tools registered: add, multiply, uppercase\n');

  console.log('───────────────────────────────────────────────────────────');
  console.log('2. PLUGINS');
  console.log('───────────────────────────────────────────────────────────\n');

  console.log('Plugin registered: DemoPlugin\n');

  console.log('───────────────────────────────────────────────────────────');
  console.log('3. HOOKS');
  console.log('───────────────────────────────────────────────────────────\n');

  console.log('Hooks registered: BeforeToolCall, AfterToolCall, BeforeLlmCall, AfterLlmCall\n');

  console.log('───────────────────────────────────────────────────────────');
  console.log('4. LLM REACT - Testing tool calls');
  console.log('───────────────────────────────────────────────────────────\n');

  console.log('Task: "What is 12 + 8? Use the add tool."\n');
  const r1 = await agent.react('What is 12 + 8? Use the add tool.');
  console.log('Result:', r1, '\n');

  console.log('Task: "What is 7 times 6? Use the multiply tool."\n');
  const r2 = await agent.react('What is 7 times 6? Use the multiply tool.');
  console.log('Result:', r2, '\n');

  console.log('───────────────────────────────────────────────────────────');
  console.log('5. SESSION MANAGEMENT');
  console.log('───────────────────────────────────────────────────────────\n');

  const session = agent.session;
  const sessionJson = session.export();
  const sessionData = JSON.parse(sessionJson);
  console.log('Messages:', sessionData.messages?.length || 0);

  const sessionFile = path.join(__dirname, '.demo-session.json');
  await session.saveFull(sessionFile);
  console.log('Session saved to:', sessionFile);

  session.compact(2, 500);
  console.log('Session compacted (keep 2, max 500 chars)');

  const compactJson = session.export();
  const compactData = JSON.parse(compactJson);
  console.log('After compact:', compactData.messages?.length, 'messages');

  await session.restoreFull(sessionFile);
  const restoredJson = session.export();
  const restoredData = JSON.parse(restoredJson);
  console.log('After restore:', restoredData.messages?.length, 'messages');

  session.clear();
  console.log('After clear: session cleared');

  fs.rmSync(sessionFile);
  console.log('Session file cleaned up\n');

  console.log('───────────────────────────────────────────────────────────');
  console.log('6. SKILLS');
  console.log('───────────────────────────────────────────────────────────\n');

  console.log('Skills loaded from:', skillsDir);

  const r4 = await agent.ask('Write a python function with type hints');
  console.log('Result:', r4.substring(0, 200), '...\n');

  fs.rmSync(skillsDir, { recursive: true });

  await brain.stop();
  console.log('───────────────────────────────────────────────────────────');
  console.log('✓ All features: Tools, Plugins, Hooks, Session, Skills!');
  console.log('───────────────────────────────────────────────────────────');
  process.exit(0);
}

main().catch((e) => {
  console.error('Error:', e.message);
  process.exit(1);
});