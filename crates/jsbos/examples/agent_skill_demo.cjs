#!/usr/bin/env node
/**
 * Agent Skills Demo — Load skills from directory and use them with LLM
 *
 * Demonstrates:
 * 1. Creating skill definitions (SKILL.md with YAML frontmatter)
 * 2. Registering skills with the agent via register_skills_from_dir()
 * 3. Running the agent — skills provide context for LLM responses
 *
 * Usage:
 *     export OPENAI_API_KEY="sk-..."
 *     node crates/jsbos/examples/agent_skill_demo.js
 */

const { Bus, Agent, ConfigLoader, version } = require('../jsbos.cjs');
const fs = require('fs');
const os = require('os');
const path = require('path');

const loader = new ConfigLoader();
loader.discover();
const _config = JSON.parse(loader.loadSync());
const _global = _config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || _global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || _global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || _global.model || 'nvidia/meta/llama-3.1-8b-instruct';

function createSampleSkills(skillsDir) {
  const pythonSkill = path.join(skillsDir, 'python-coding');
  fs.mkdirSync(pythonSkill, { recursive: true });
  fs.writeFileSync(path.join(pythonSkill, 'SKILL.md'), `---
name: python-coding
description: Python coding conventions and best practices for the project
category: coding
version: 1.0.0
author: team
tags: [python, style, conventions]
---

# Python Coding Conventions

## Naming
- Use snake_case for functions and variables
- Use PascalCase for classes
- Use UPPER_CASE for constants

## Error Handling
- Always use specific exception types, never bare \`except:\`
- Log errors with context before re-raising
- Use context managers for resource cleanup

## Type Hints
- All function signatures must have type hints
- Use \`Optional[T]\` instead of \`T | None\` for Python 3.9 compatibility
- Return types are mandatory

## Testing
- One test per function
- Use pytest fixtures for setup
- Mock external services
`);

  const dbSkill = path.join(skillsDir, 'database-ops');
  fs.mkdirSync(dbSkill, { recursive: true });
  fs.writeFileSync(path.join(dbSkill, 'SKILL.md'), `---
name: database-ops
description: Database query patterns and connection management
category: infrastructure
version: 1.0.0
author: team
tags: [database, sql, connection]
requires: []
provides: [db-queries, db-connections]
---

# Database Operations

## Connection Pool
- Use connection pooling (max 10 connections)
- Set connection timeout to 30 seconds
- Always return connections to pool after use

## Query Patterns
- Use parameterized queries to prevent SQL injection
- Batch inserts for >100 rows
- Use transactions for multi-table operations

## Error Recovery
- Retry transient errors with exponential backoff
- Max 3 retries before failing
- Log query parameters (excluding secrets) on failure
`);

  const apiSkill = path.join(skillsDir, 'api-design');
  fs.mkdirSync(apiSkill, { recursive: true });
  fs.writeFileSync(path.join(apiSkill, 'SKILL.md'), `---
name: api-design
description: REST API design patterns and conventions
category: architecture
version: 1.0.0
author: team
tags: [api, rest, design]
---

# API Design Patterns

## Endpoints
- Use nouns for resources: \`/users\`, \`/orders\`
- Use HTTP methods correctly: GET, POST, PUT, DELETE
- Version APIs in URL: \`/v1/users\`

## Responses
- Always return consistent JSON structure
- Include pagination metadata for list endpoints
- Use proper HTTP status codes

## Authentication
- Use JWT tokens in Authorization header
- Token expiry: 1 hour
- Refresh tokens valid for 7 days
`);
}

async function demoSkills() {
  console.log('═'.repeat(60));
  console.log('  Demo — Agent with Skills');
  console.log('═'.repeat(60));

  const skillsDir = fs.mkdtempSync(path.join(os.tmpdir(), 'brainos_skills_'));
  console.log(`\n  📁 Creating skills in: ${skillsDir}`);
  createSampleSkills(skillsDir);

  const bus = await Bus.create();
  const agent = await Agent.create({
    name: 'skill-agent',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt:
      'You are a helpful assistant. ' +
      'Use the loaded skills as context when answering questions. ' +
      'Reference specific skill instructions when relevant. ' +
      'Format: Thought: <reasoning>\nFinal Answer: <response>',
    temperature: 0.7,
    timeoutSecs: 120,
  }, bus);
  console.log('  🤖 Agent created');

  await agent.registerSkillsFromDir(skillsDir);
  console.log('  📚 Skills registered from directory');

  const prompts = [
    ['Python Style', 'What are the Python naming conventions for this project?'],
    ['Database', 'How should I handle database connections and retries?'],
    ['API Design', "What's the correct way to version our REST API?"],
  ];

  for (const [label, prompt] of prompts) {
    console.log(`\n  ── ${label} ──`);
    console.log(`  📤 User: ${prompt}`);
    try {
      const reply = await agent.react(prompt);
      console.log(`  📥 Agent: ${reply.substring(0, 300)}`);
    } catch (e) {
      console.log(`  ⚠️  ${e.message}`);
    }
  }

  fs.rmSync(skillsDir, { recursive: true, force: true });
  console.log('\n  ✅ Skills demo done\n');
}

async function main() {
  console.log('\n' + '📚'.repeat(30));
  console.log('  BrainOS — Agent Skills Demo');
  console.log('📚'.repeat(30) + '\n');

  if (!API_KEY) {
    console.log('  ⚠️  OPENAI_API_KEY not set — LLM calls will fail');
    console.log('  Set: export OPENAI_API_KEY=sk-...\n');
  }

  await demoSkills();

  console.log('═'.repeat(60));
  console.log('  ✅ All Skills demos completed!');
  console.log('═'.repeat(60) + '\n');
}

main().catch(console.error).finally(() => process.exit(0));
