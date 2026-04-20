const test = require('ava')
const { version, ConfigLoader, Bus, Agent } = require('../index.js');

test('can create config loader', t => {
  const loader = new ConfigLoader()
  t.truthy(loader)
})


const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';

async function main() {
  console.log('jsbos version:', version());

  const loader = new ConfigLoader();
  loader.discover();
  const config = JSON.parse(loader.loadSync());
  console.log('Config loaded:', Object.keys(config));

  const bus = await Bus.create();
  console.log('Bus created successfully');

  const agent = await Agent.create({
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    model: MODEL,
    name: 'test-agent',
    systemPrompt: 'You are a test agent.',
    temperature: 0.7,
    timeoutSecs: 60,
  }, bus);
  console.log('Agent created:', agent.config());
  console.log('All tests passed!');
}

main().catch(console.error);