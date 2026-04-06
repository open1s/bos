#!/usr/bin/env node
/**
 * MCP HTTP Transport Demo — Connect to MCP servers via Streamable HTTP
 *
 * Demonstrates:
 * 1. Connecting via McpClient.connect_http() (no process spawning)
 * 2. Listing and calling tools over HTTP
 * 3. Comparing Stdio vs HTTP transport
 *
 * Note: This demo shows HTTP transport usage. For full demo with local server,
 * you would need to run mcp_http_server.py first, then connect to it.
 *
 * Usage:
 *     node crates/jsbos/examples/mcp_demo.js
 *     # First run: python3 crates/examples/mcp_http_server.py
 *     # Then this demo can connect to http://127.0.0.1:8765/mcp
 */

const { McpClient, version } = require('../jsbos.cjs');

async function demoHttpConnect() {
  console.log('═'.repeat(60));
  console.log('  Demo 1 — HTTP transport connect');
  console.log('═'.repeat(60));

  // This demonstrates connecting to an HTTP MCP server
  // To test fully, first run: python3 crates/examples/mcp_http_server.py
  const httpUrl = 'http://127.0.0.1:8765/mcp';
  
  try {
    const client = McpClient.connectHttp(httpUrl);
    console.log(`  🔗 HTTP client created for: ${httpUrl}`);

    const caps = await client.initialize();
    console.log(`  📋 Initialized — server info available`);
    console.log(`     capabilities: tools=${!!caps.tools}, resources=${!!caps.resources}`);

    const tools = await client.listTools();
    console.log(`  🔧 Available tools: ${tools.length}`);
    for (const t of tools) {
      console.log(`     - ${t.name}: ${(t.description || '').slice(0, 50)}`);
    }

    if (tools.length > 0) {
      const toolName = tools[0].name;
      const result = await client.callTool(toolName, '{}');
      console.log(`  📤 Called: ${toolName}`);
      console.log(`  📥 Result: ${JSON.stringify(result).slice(0, 200)}`);
    }

    console.log('  ✅ HTTP connect demo done\n');

  } catch (e) {
    console.log(`  ℹ️  Server not running at ${httpUrl}`);
    console.log(`      Run: python3 crates/examples/mcp_http_server.py`);
    console.log(`      Then re-run this demo\n`);
  }
}

async function demoHttpVsStdio() {
  console.log('═'.repeat(60));
  console.log('  Demo 2 — Stdio vs HTTP transport comparison');
  console.log('═'.repeat(60));

  console.log('\n  ── HTTP transport (demonstrated above) ──');
  console.log('  📋 connectHttp(url) - no process spawning, stateless');
  console.log('  📋 Good for: cloud services, remote MCP servers');

  console.log('\n  ── Stdio transport ──');
  console.log('  📋 spawn(command, args) - spawns local process');
  console.log('  📋 Good for: local MCP servers, development');

  console.log('\n  ✅ Transport comparison done\n');
}

async function main() {
  console.log('\n' + '🌐'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — MCP HTTP Transport Demo`);
  console.log('🌐'.repeat(30) + '\n');

  await demoHttpConnect();
  await demoHttpVsStdio();

  console.log('═'.repeat(60));
  console.log('  ✅ All HTTP MCP demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));