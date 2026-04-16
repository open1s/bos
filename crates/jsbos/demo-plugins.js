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
    name: agentCfg.name || 'plugin-demo',
    model: globalCfg.model || 'gpt-4.1',
    apiKey: globalCfg.api_key || '',
    baseUrl: globalCfg.base_url || 'https://api.openai.com/v1',
    systemPrompt: agentCfg.system_prompt || 'You are a helpful assistant.',
    temperature: agentCfg.temperature ?? 0.7,
    timeoutSecs: agentCfg.timeout_secs ?? 120,
  };
}


async function main() {
  console.log('JSBOS Plugin Demo');
  console.log('='.repeat(50));

  const config = await loadConfig();
  console.log(`\nModel: ${config.model}`);

  const agent = await Agent.create(config);

  console.log('\n--- Registering plugin with all 4 intercept points ---');

  agent.registerPlugin(
    'DemoInterceptor',
    (err, request) => {
      console.log(`  [Plugin:on_llm_request] model=${request.model}, temp=${request.temperature}`);
      return JSON.stringify(request);
    },
    (err, response) => {
      console.log(`  [Plugin:on_llm_response] type=${response.type}, content=${response.content || '(none)'}`);
      return JSON.stringify(response);
    },
    (err, toolCall) => {
      console.log(`  [Plugin:on_tool_call] name=${toolCall.name}, args=${JSON.stringify(toolCall.args)}`);
      return JSON.stringify(toolCall);
    },
    (err, toolResult) => {
      console.log(`  [Plugin:on_tool_result] success=${toolResult.success}, result=${JSON.stringify(toolResult.result)}`);
      return JSON.stringify(toolResult);
    },
  );

  console.log('Plugin registered with all 4 intercept points\n');

  console.log('--- Also registering all 7 hook events ---');
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

  const hookNames = [
    'BeforeLlmCall',
    'AfterLlmCall',
    'BeforeToolCall',
    'AfterToolCall',
    'OnMessage',
    'OnComplete',
    'OnError',
  ];

  for (let i = 0; i < hookEvents.length; i++) {
    const event = hookEvents[i];
    const name = hookNames[i];
    await agent.registerHook(event, async (ctx) => {
      hookLog.push(name);
      console.log(`  [Hook:${name}] fired`);
      return 'Continue';
    });
  }
  console.log('All 7 hooks registered\n');

  console.log('--- Registering "add" tool for react() test ---');
  await agent.addTool(
    'add',
    'Add two numbers together',
    'a: number, b: number',
    JSON.stringify({
      type: 'object',
      properties: {
        a: { type: 'number', description: 'First number' },
        b: { type: 'number', description: 'Second number' },
      },
      required: ['a', 'b'],
    }),
    (err, args) => {
      const result = (args.a || 0) + (args.b || 0);
      return JSON.stringify(result);
    },
  );
  console.log('Tool "add" registered\n');

  console.log('=== TEST 1: run_simple() — LLM plugins + hooks ===');
  console.log('Running: agent.runSimple("What is 2+2?")\n');
  try {
    const result = await agent.runSimple('What is 2+2?');
    console.log(`\nResult: ${result}`);
  } catch (e) {
    console.log(`\nError: ${e.message || e}`);
  }
  console.log(`\nHooks fired (${hookLog.length}):`);
  hookLog.forEach(h => console.log(`  - ${h}`));

  console.log('\n=== TEST 2: react() with tool — all 4 plugin intercepts ===');
  hookLog.length = 0;
  console.log('Running: agent.react("What is 5+3? Use the add tool.")\n');
  try {
    const result = await agent.react('What is 5+3? Use the add tool.');
    console.log(`\nResult: ${result}`);
  } catch (e) {
    console.log(`\nError: ${e.message || e}`);
  }
  console.log(`\nHooks fired (${hookLog.length}):`);
  hookLog.forEach(h => console.log(`  - ${h}`));

  console.log('\n=== TEST 3: Error handling — OnError hook ===');
  hookLog.length = 0;
  const badAgent = await Agent.create({ ...config, apiKey: 'invalid-key' });
  for (let i = 0; i < hookEvents.length; i++) {
    const event = hookEvents[i];
    const name = hookNames[i];
    await badAgent.registerHook(event, async (ctx) => {
      hookLog.push(name);
      console.log(`  [Hook:${name}] fired`);
      return 'Continue';
    });
  }
  console.log('Running with invalid API key...\n');
  try {
    await badAgent.runSimple('test');
  } catch (e) {
    console.log(`Error (expected): ${e.message || e}`);
  }
  console.log(`\nHooks fired (${hookLog.length}):`);
  hookLog.forEach(h => console.log(`  - ${h}`));

  badAgent.close();
  agent.close();
}


main()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(err);
    process.exit(1);
  });
