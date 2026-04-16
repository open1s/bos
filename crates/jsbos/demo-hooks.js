const { 
  Agent, 
  HookEvent, 
  ConfigLoader,
} = require('./index.js');


async function loadConfig() {
  const loader = new ConfigLoader();
  loader.discover();
  const cfg = JSON.parse(loader.loadSync());
  const agentCfg = cfg.agent || {};
  const globalCfg = cfg.global_model || {};
  
  return {
    name: agentCfg.name || 'assistant',
    model: globalCfg.model || 'gpt-4.1',
    apiKey: globalCfg.api_key || '',
    baseUrl: globalCfg.base_url || 'https://api.openai.com/v1',
    systemPrompt: agentCfg.system_prompt || 'You are a helpful assistant.',
    temperature: agentCfg.temperature ?? 0.7,
    timeoutSecs: agentCfg.timeout_secs ?? 120,
  };
}


async function main() {
  console.log('JSBOS Hook & Plugin Full Demo');
  console.log('='.repeat(50));
  
  const config = await loadConfig();
  console.log(`\nModel: ${config.model}`);
  
  const agent = await Agent.create(config);
  
  console.log('\n--- Registering all 7 hook events ---');
  const hookLog = [];
  
  const hookEvents = [
    HookEvent.BeforeLlmCall,
    HookEvent.AfterLlmCall,
    HookEvent.BeforeToolCall,
    HookEvent.AfterToolCall,
    HookEvent.OnMessage,
    HookEvent.OnComplete,
    HookEvent.OnError,
  ];
  
  for (const event of hookEvents) {
    await agent.registerHook(event, async (ctx) => {
      hookLog.push(event);
      console.log(`  [${event}] fired`);
      return 'Continue';
    });
  }
  console.log('All 7 hooks registered');
  
  console.log('\n=== TEST 1: run_simple() ===');
  hookLog.length = 0;
  console.log('\nRunning: agent.runSimple("What is 2+2?")');
  try {
    const result = await agent.runSimple('What is 2+2?');
    console.log(`Result: ${result}`);
  } catch (e) {
    console.log(`Error: ${e.message || e}`);
  }
  console.log(`\nHooks fired: ${hookLog.length}`);
  hookLog.forEach(h => console.log(`  - ${h}`));
  
  console.log('\n=== TEST 2: Error handling ===');
  hookLog.length = 0;
  const badConfig = { ...config, apiKey: 'invalid-key' };
  const badAgent = await Agent.create(badConfig);
  
  for (const event of hookEvents) {
    await badAgent.registerHook(event, async (ctx) => {
      hookLog.push(event);
      console.log(`  [${event}] fired`);
      return 'Continue';
    });
  }
  
  console.log('\nRunning with invalid API key...');
  try {
    const result = await badAgent.runSimple('test');
    console.log(`Result: ${result}`);
  } catch (e) {
    console.log(`Error (expected): ${e.message || e}`);
  }
  console.log(`\nHooks fired: ${hookLog.length}`);
  hookLog.forEach(h => console.log(`  - ${h}`));
  
  console.log('\n' + '='.repeat(50));
  console.log('SUMMARY');
  console.log('='.repeat(50));
  console.log(`
Hook Events:
  - BeforeLlmCall: Before sending request to LLM
  - AfterLlmCall: After receiving response from LLM
  - BeforeToolCall: Before executing a tool
  - AfterToolCall: After tool execution completes
  - OnMessage: When message is added to conversation
  - OnComplete: When agent completes successfully
  - OnError: When an error occurs

Hook Decisions:
  - Continue: Proceed normally
  - Abort: Stop current operation
  - Error(msg): Return error to caller
`);

  badAgent.close();
  agent.close();
}


main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(err);
    process.exit(1);
  });
