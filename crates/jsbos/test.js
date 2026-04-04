const { version, ConfigLoader, Bus, Agent } = require('./index.js');

async function main() {
  console.log('jsbos version:', version());

  const loader = new ConfigLoader();
  loader.discover();
  const config = JSON.parse(loader.loadSync());
  console.log('Config loaded:', Object.keys(config));

  const bus = await Bus.create();
  console.log('Bus created successfully');

  const agent = await Agent.create({
    name: 'test-agent',
    model: 'nvidia/meta/llama-3.1-8b-instruct',
    systemPrompt: 'You are a test agent.',
    temperature: 0.7,
  }, bus);
  console.log('Agent created:', agent.config());
  console.log('All tests passed!');
}

main().catch(console.error);