const { BrainOS, HookEvent } = require('./brainos.js');

async function demo() {
  console.log('=== JSBOS BrainOS Hook Demo ===\n');

  const brain = new BrainOS({
    apiKey: process.env.OPENAI_API_KEY || 'sk-test',
    model: 'gpt-4',
  });

  await brain.start();

  console.log('1. Adding hooks via onHook()...');
  
  brain.agent('assistant').onHook(HookEvent.BeforeToolCall, (ctx) => {
    console.log('   [BeforeToolCall]', ctx.data.tool_name || 'unknown');
    return 'continue';
  });
  
  brain.agent('assistant').onHook(HookEvent.AfterToolCall, (ctx) => {
    console.log('   [AfterToolCall]', ctx.data.tool_name || 'unknown');
    return 'continue';
  });
  
  brain.agent('assistant').onHook(HookEvent.BeforeLlmCall, (ctx) => {
    console.log('   [BeforeLlmCall] Starting LLM call');
    return 'continue';
  });
  
  brain.agent('assistant').onHook(HookEvent.AfterLlmCall, (ctx) => {
    console.log('   [AfterLlmCall] LLM call completed');
    return 'continue';
  });
  
  brain.agent('assistant').onHook(HookEvent.OnError, (ctx) => {
    console.log('   [OnError]', ctx.data.error || 'unknown error');
    return 'continue';
  });

  console.log('   Hooks registered!\n');

  console.log('2. Hook decisions:');
  console.log('   - "continue" or return nothing: proceed normally');
  console.log('   - "abort": abort the current operation');
  console.log('   - "error:message": return an error\n');

  console.log('3. Running agent (hooks will fire during execution)...');
  try {
    const result = await brain.agent('assistant').ask('Say "test" in one word');
    console.log('   Result:', result.substring(0, 100) + '...');
  } catch (e) {
    console.log('   (Expected - using dummy API key)');
  }

  console.log('\n=== Done ===');
}

demo().catch(console.error);
