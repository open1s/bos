#!/usr/bin/env node
/**
 * MCP HTTP Server Agent Demo — Connect agent to MCP server via HTTP
 *
 * Usage:
 *     node examples/mcp_http_agent_demo.cjs
 *     # Requires an MCP HTTP server running at http://127.0.0.1:8000/mcp
 */

const { Agent, Bus,ConfigLoader, McpClient, version,initTracing } = require('../index.js');
initTracing();

const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';


async function demoAgentHttpMcp() {
  console.log('═'.repeat(60));
  console.log('  Demo — Agent + MCP HTTP Server');
  console.log('═'.repeat(60));

  const bus = await Bus.create();
  const agent = await Agent.create({
    name: 'http-mcp-agent',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a helpful assistant. ' +
      'When asked to use a tool, output ONLY the tool call. ' +
      'Use format: namespace/tool_name(args)',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus);
  console.log('  🤖 Agent created');

  const MCP_URL = 'http://127.0.0.1:8000/mcp';

  try {
    console.log(`  🔗 Connecting to MCP HTTP server: ${MCP_URL}`);

    await agent.addMcpServerHttp('http', MCP_URL);
    console.log("  🔌 MCP HTTP server connected");

    const mcpTools = await agent.listMcpTools();
    console.log(`  🔧 MCP tools registered: ${mcpTools.length}`);
    for (const t of mcpTools) {
      console.log(`     - ${t.name}: ${(t.description || '').slice(0, 50)}`);
    }

    if (mcpTools.length > 0) {
      const prompts = [
        ['First', mcpTools[0].description ? `Use the ${mcpTools[0].name} tool` : 'List your available tools'],
      ];

      for (const [label, prompt] of prompts) {
        console.log(`\n  [${label}] User: ${prompt}`);
        try {
          const reply = await agent.react(prompt);
          console.log(`  [${label}] Agent: ${reply.substring(0, 300)}`);
        } catch (e) {
          console.log(`  [${label}] ⚠️  ${e.message}`);
        }
      }
    }

    console.log('\n  ✅ MCP HTTP Agent demo done\n');

  } catch (e) {
    console.log(`  ❌ Failed: ${e.message}`);
    console.log(`\n  Make sure an MCP HTTP server is running at ${MCP_URL}`);
    console.log(`  Example server: npx -y mcp-hello-world@latest --http\n`);
  }
}

async function demoStandaloneHttpClient() {
  console.log('═'.repeat(60));
  console.log('  Demo — Standalone McpClient HTTP');
  console.log('═'.repeat(60));

  const MCP_URL = 'http://127.0.0.1:8000/mcp';

  try {
    console.log(`  🔗 Connecting to MCP HTTP server: ${MCP_URL}`);

    const client = McpClient.connectHttp(MCP_URL);
    await client.initialize();

    const tools = await client.listTools();
    console.log(`  🔧 Available tools: ${tools.length}`);
    for (const t of tools) {
      console.log(`     - ${t.name}: ${(t.description || '').slice(0, 50)}`);
    }

    if (tools.length > 0) {
      const tool = tools[0];
      console.log(`\n  📤 Calling tool: ${tool.name}`);
      const result = await client.callTool(tool.name, JSON.stringify({}));
      console.log(`  📥 Result: ${JSON.stringify(result).slice(0, 200)}`);
    }

    console.log('\n  ✅ Standalone MCP HTTP demo done\n');

  } catch (e) {
    console.log(`  ℹ️  Server not running at ${MCP_URL}`);
    console.log(`      Error: ${e.message}\n`);
  }
}

async function main() {
  console.log('\n' + '🌐'.repeat(30));
  console.log(`  BrainOS (jsbos v${version()}) — MCP HTTP Demo`);
  console.log('🌐'.repeat(30) + '\n');

  await demoStandaloneHttpClient();
  await demoAgentHttpMcp();

  console.log('═'.repeat(60));
  console.log('  ✅ All MCP HTTP demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));