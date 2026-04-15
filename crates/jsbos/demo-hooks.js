const { HookEvent, HookDecision, HookContextData, HookRegistry } = require('./index.js');

async function demo() {
  console.log('=== JSBOS Hook Demo ===\n');

  const registry = new HookRegistry();
  
  await registry.register(HookEvent.BeforeToolCall, async (ctx) => {
    console.log('[BeforeToolCall]', ctx.agent_id);
    return 'Continue';
  });

  await registry.register(HookEvent.AfterToolCall, async (ctx) => {
    console.log('[AfterToolCall]', ctx.agent_id);
    return 'Continue';
  });

  console.log('Events:', 'BeforeToolCall', 'AfterToolCall', 'BeforeLlmCall', 'AfterLlmCall', 'OnError');
  console.log('Decisions:', 'Continue', 'Abort', 'Error(msg)');
  console.log('\n=== Done ===');
}

demo().catch(console.error);
