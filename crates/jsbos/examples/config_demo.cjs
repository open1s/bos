#!/usr/bin/env node
/**
 * Config Demo — Discovery and loading
 *
 * Demonstrates:
 * 1. Config discover (auto-find ~/.bos/conf/config.toml)
 * 2. Adding files and inline overrides
 * 3. Loading and reloading config
 *
 * Usage:
 *     node crates/jsbos/examples/config_demo.js
 */

const { ConfigLoader, version } = require('../index.js');

function demoDiscover() {
  console.log('═'.repeat(60));
  console.log('  Demo 1 — Config discover');
  console.log('═'.repeat(60));

  const cfg = new ConfigLoader();
  cfg.discover();
  const data = JSON.parse(cfg.loadSync());

  if (Object.keys(data).length > 0) {
    console.log(`  📄 Loaded config with keys: ${Object.keys(data)}`);
    if (data.global_model) {
      console.log(`  🤖 Model: ${data.global_model.model || 'N/A'}`);
    }
    if (data.bus) {
      console.log(`  🚌 Bus mode: ${data.bus.mode || 'N/A'}`);
    }
  } else {
    console.log('  ℹ️  No config file found (create ~/.bos/conf/config.toml)');
  }
  console.log();
}

function demoInlineOverride() {
  console.log('═'.repeat(60));
  console.log('  Demo 2 — Inline config override');
  console.log('═'.repeat(60));

  const cfg = new ConfigLoader();
  cfg.addInline({
    agent: {
      name: 'demo-agent',
      model: 'gpt-4',
      temperature: 0.5,
    },
    features: ['chat', 'tools'],
  });
  const data = JSON.parse(cfg.loadSync());
  console.log(`  📄 Agent name: ${data.agent.name}`);
  console.log(`  📄 Agent model: ${data.agent.model}`);
  console.log(`  📄 Features: ${data.features}`);
  console.log();
}

function demoReload() {
  console.log('═'.repeat(60));
  console.log(' Demo 3 — Config reload');
  console.log('═'.repeat(60));

  const cfg = new ConfigLoader();
  cfg.addInline({ version: 1 });
  const initial = JSON.parse(cfg.loadSync());
  console.log(` 📄 Initial: ${JSON.stringify(initial)}`);

  cfg.reset();
  cfg.addInline({ version: 2, new_key: 'added' });
  const reloaded = JSON.parse(cfg.loadSync());
  console.log(` 📄 After reload: ${JSON.stringify(reloaded)}`);
  console.log();
}

function main() {
  console.log('\n' + '⚙️'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — Config Demo`);
  console.log('⚙️'.repeat(30) + '\n');

  demoDiscover();
  demoInlineOverride();
  demoReload();

  console.log('═'.repeat(60));
  console.log('  ✅ All Config demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main();
