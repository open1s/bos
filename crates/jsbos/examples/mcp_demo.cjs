#!/usr/bin/env node
/**
 * MCP Client Demo — Connect to an MCP server and interact with its tools
 *
 * Demonstrates:
 * 1. Spawning an MCP server process
 * 2. Initializing the MCP connection
 * 3. Listing available tools, resources, and prompts
 * 4. Calling an MCP tool with arguments
 *
 * Prerequisites:
 *     npm installed (for npx)
 *     Or use any MCP server binary path
 *
 * Usage:
 *     node crates/jsbos/examples/mcp_demo.js
 */

const { McpClient, version } = require('../jsbos.cjs');

function sleep(ms) {
  return new Promise(resolve => setTimeout(resolve, ms));
}

async function demoMcpHelloWorld() {
  console.log('═'.repeat(60));
  console.log('  Demo 1 — MCP Hello World server');
  console.log('═'.repeat(60));

  try {
    const client = await McpClient.spawn('npx', ['-y', 'mcp-hello-world@latest']);
    console.log('  🚀 MCP server spawned');

    const caps = await client.initialize();
    console.log(`  📋 Capabilities: ${JSON.stringify(caps).slice(0, 200)}`);

    const tools = await client.listTools();
    console.log(`  🔧 Available tools: ${tools.length}`);
    for (const t of tools) {
      console.log(`     - ${t.name}: ${(t.description || '').slice(0, 60)}`);
    }

    if (tools.length > 0) {
      const toolName = tools[0].name;
      const args = toolName === 'echo' ? JSON.stringify({ message: 'from brainos' }) : '{}';
      const result = await client.callTool(toolName, args);
      console.log(`  📤 Called: ${toolName}(${args})`);
      console.log(`  📥 Result: ${JSON.stringify(result).slice(0, 200)}`);

      const addTool = tools.find(t => t.name === 'add');
      if (addTool) {
        const addResult = await client.callTool('add', JSON.stringify({ a: 3, b: 4 }));
        console.log('  📤 Called: add(3, 4)');
        console.log(`  📥 Result: ${JSON.stringify(addResult).slice(0, 200)}`);
      }
    }

    const prompts = await client.listPrompts();
    if (prompts.length > 0) {
      console.log(`  💬 Prompts: ${prompts.length}`);
      for (const p of prompts) {
        console.log(`     - ${p.name}`);
      }
    }

    const resources = await client.listResources();
    if (resources.length > 0) {
      console.log(`  📁 Resources: ${resources.length}`);
      for (const r of resources) {
        console.log(`     - ${r.uri}: ${r.name || ''}`);
      }
    }

    console.log('  ✅ MCP Hello World demo done\n');

  } catch (e) {
    if (e.message && e.message.includes('spawn')) {
      console.log('  ℹ️  npx not found — install Node.js to run this demo\n');
    } else {
      console.log(`  ⚠️  ${e}\n`);
    }
  }
}

async function demoMcpFilesystem() {
  const os = require('os');
  const home = os.homedir();

  try {
    console.log('═'.repeat(60));
    console.log('  Demo 2 — MCP Filesystem server');
    console.log('═'.repeat(60));

    const client = await McpClient.spawn('npx', ['-y', '@modelcontextprotocol/server-filesystem@latest', home]);
    console.log(`  🚀 MCP filesystem server spawned (root: ${home})`);

    const caps = await client.initialize();
    console.log('  📋 Initialized');

    const tools = await client.listTools();
    console.log(`  🔧 Tools: ${tools.map(t => t.name).join(', ')}`);

    const listDirTool = tools.find(t => t.name === 'list_directory');
    if (listDirTool) {
      const result = await client.callTool('list_directory', JSON.stringify({ path: home }));
      const entries = result.content || [];
      const names = entries.slice(0, 5).map(e => (e.text || '').slice(0, 60));
      console.log(`  📁 list_directory('${home}'): ${names.join(', ')}`);
    }

    try {
      const resources = await client.listResources();
      if (resources.length > 0) {
        console.log(`  📁 Found ${resources.length} resources`);
        for (const r of resources.slice(0, 3)) {
          console.log(`     - ${r.uri}`);
        }
        if (resources.length > 0) {
          const uri = resources[0].uri;
          const readResult = await client.readResource(uri);
          const contents = readResult.contents || [];
          if (contents.length > 0) {
            const text = contents[0].text || '';
            console.log(`  📄 Read ${uri}: ${text.slice(0, 120)}...`);
          }
        }
      }
    } catch (e) {
      console.log('  ℹ️  Server does not support resources — skipping');
    }

    console.log('  ✅ MCP Filesystem demo done\n');

  } catch (e) {
    if (e.message && e.message.includes('spawn')) {
      console.log('  ℹ️  npx not found — install Node.js to run this demo\n');
    } else {
      console.log(`  ⚠️  ${e}\n`);
    }
  }
}

async function main() {
  console.log('\n' + '🔌'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — MCP Client Demo`);
  console.log('🔌'.repeat(30) + '\n');

  await demoMcpHelloWorld();
  await demoMcpFilesystem();

  console.log('═'.repeat(60));
  console.log('  ✅ All MCP demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));