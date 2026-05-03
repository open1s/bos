#!/usr/bin/env node
/**
 * LLM Performance Harness
 *
 * Measures: E2E latency, throughput concurrency, framework overhead,
 * streaming TTFT/inter-token, and resilience behavior.
 *
 * Usage: node performance_harness.cjs [--quick]
 *   --quick: reduces test iterations for rapid feedback
 */

const { Agent, Bus, ConfigLoader, version } = require('../index.js');

const loader = new ConfigLoader();
loader.discover();
const config = JSON.parse(loader.loadSync());
const global = config.global_model || {};

const API_KEY = process.env.OPENAI_API_KEY || global.api_key || '';
const BASE_URL = process.env.LLM_BASE_URL || global.base_url || 'https://integrate.api.nvidia.com/v1';
const MODEL = process.env.LLM_MODEL || global.model || 'nvidia/z-ai/glm4.7';

const QUICK = process.argv.includes('--quick');
const WARMUP = QUICK ? 1 : 2;
const ITERATIONS = QUICK ? 3 : 10;
const CONCURRENCY_LEVELS = QUICK ? [1] : [1, 2, 4];
const TIMEOUT = 120000;

async function createAgent(bus) {
  return Agent.create({
    name: 'perf-test',
    model: MODEL,
    baseUrl: BASE_URL,
    apiKey: API_KEY,
    systemPrompt: 'Answer briefly in 1-2 sentences.',
    temperature: 0,
    timeoutSecs: 60,
  }, bus);
}

async function warmup(agent) {
  for (let i = 0; i < WARMUP; i++) {
    await agent.runSimple('Say: ok');
  }
}

// ── A: End-to-End Latency ──
async function withTimeout(promise, ms) {
  return Promise.race([
    promise,
    new Promise((_, reject) => setTimeout(() => reject(new Error('timeout')), ms))
  ]);
}

async function benchmarkE2E(agent) {
  console.log('\n' + '═'.repeat(60));
  console.log('  A — End-to-End Latency');
  console.log('═'.repeat(60));

  const times = [];
  for (let i = 0; i < ITERATIONS; i++) {
    try {
      const start = process.hrtime.bigint();
      await withTimeout(agent.runSimple('What is 2 + 3?'), 45000);
      const end = process.hrtime.bigint();
      times.push(Number(end - start) / 1e6);
      process.stdout.write('.');
    } catch (e) {
      process.stdout.write('x');
    }
  }

  if (times.length === 0) {
    console.log('  No successful runs');
    return null;
  }

  const sorted = [...times].sort((a, b) => a - b);
  const sum = times.reduce((a, b) => a + b, 0);

  console.log(`\n  P50: ${sorted[Math.floor(sorted.length * 0.5)].toFixed(1)}ms`);
  console.log(`  P95: ${sorted[Math.floor(sorted.length * 0.95)].toFixed(1)}ms`);
  console.log(`  P99: ${sorted[Math.floor(sorted.length * 0.99)].toFixed(1)}ms`);
  console.log(`  Avg: ${(sum / times.length).toFixed(1)}ms`);
  console.log(`  Min: ${sorted[0].toFixed(1)}ms`);
  console.log(`  Max: ${sorted[sorted.length - 1].toFixed(1)}ms`);

  return { p50: sorted[Math.floor(sorted.length * 0.5)], avg: sum / times.length, times };
}

// ── B: Throughput (Concurrency) ──
async function benchmarkThroughput(agent) {
  console.log('\n' + '═'.repeat(60));
  console.log('  B — Throughput Under Load');
  console.log('═'.repeat(60));

  const results = {};

  for (const concurrency of CONCURRENCY_LEVELS) {
    const tasks = Array(concurrency).fill(null).map(() =>
      agent.runSimple('Say: pong').catch(() => '(error)')
    );

    const start = process.hrtime.bigint();
    await Promise.all(tasks);
    const elapsed = Number(process.hrtime.bigint() - start) / 1e6;

    const rate = (concurrency / (elapsed / 1000)).toFixed(1);
    results[concurrency] = { concurrency, elapsed, rate };
    console.log(`  Concurrency ${concurrency}: ${elapsed.toFixed(0)}ms (${rate} req/s)`);
  }

  return results;
}

// ── C: Framework Overhead ──
async function benchmarkOverhead(agent) {
  console.log('\n' + '═'.repeat(60));
  console.log('  C — Framework Overhead');
  console.log('═'.repeat(60));

  agent.resetPerfMetrics();

  let ok = 0;
  let fail = 0;
  for (let i = 0; i < ITERATIONS; i++) {
    try {
      await withTimeout(agent.runSimple('Say: hello'), 45000);
      ok++;
    } catch (e) {
      fail++;
      process.stdout.write('x');
    }
    process.stdout.write('.');
  }
  console.log(`  (${ok} ok, ${fail} failed)`);

  const m = agent.getPerfMetrics();
  console.log(`  Call count:        ${m.callCount}`);
  console.log(`  Total wall time:   ${(m.totalWallTimeUs / 1e3).toFixed(1)}ms`);
  console.log(`  Avg wall time:     ${(m.avgWallTimeUs / 1e3).toFixed(1)}ms`);
  console.log(`  Total engine time: ${(m.totalEngineTimeUs / 1e3).toFixed(1)}ms`);
  console.log(`  Resilience time:   ${(m.totalResilienceTimeUs / 1e3).toFixed(1)}ms`);
  console.log(`  Rate limit waits:  ${m.rateLimitWaits}`);
  console.log(`  Rate wait time:    ${(m.totalRateLimitWaitUs / 1e3).toFixed(1)}ms`);
  console.log(`  Circuit trips:     ${m.circuitTrips}`);
  console.log(`  LLM errors:        ${m.llmErrors}`);
  console.log(`  Tool calls:        ${m.toolCallCount}`);
  console.log(`  Tool time:         ${(m.totalToolTimeUs / 1e3).toFixed(1)}ms`);
  console.log(`  Input tokens:      ${m.totalInputTokens}`);
  console.log(`  Output tokens:     ${m.totalOutputTokens}`);

  if (m.avgWallTimeUs > 0) {
    const overhead = (m.totalWallTimeUs - m.totalEngineTimeUs) / 1e3;
    console.log(`  Non-engine overhead: ${overhead.toFixed(1)}ms`);
  }

  return m;
}

// ── D: Streaming Performance ──
async function benchmarkStreaming(agent) {
  console.log('\n' + '═'.repeat(60));
  console.log('  D — Streaming Performance');
  console.log('═'.repeat(60));

  const ttfts = [];
  const interTokenGaps = [];
  let totalTokens = 0;
  let ok = 0;
  let fail = 0;

  for (let i = 0; i < ITERATIONS; i++) {
    try {
      let firstToken = true;
      let lastTokenTime = null;
      let done = false;

      await new Promise((resolve, reject) => {
        const startTime = process.hrtime.bigint();

        agent.stream('Say: ok', (err, token) => {
          if (err) {
            if (!done) { done = true; reject(err); }
            return;
          }

          const now = process.hrtime.bigint();

          if (token.type === 'Text') {
            if (firstToken) {
              ttfts.push(Number(now - startTime) / 1e6);
              firstToken = false;
            } else if (lastTokenTime) {
              interTokenGaps.push(Number(now - lastTokenTime) / 1e6);
            }
            lastTokenTime = now;
          } else if (token.type === 'Done') {
            if (!done) {
              done = true;
              totalTokens++;
              resolve();
            }
          }
        });
      });
      ok++;
      process.stdout.write('.');
    } catch (e) {
      fail++;
      process.stdout.write('x');
    }
  }

  console.log(`  (${ok} ok, ${fail} failed)`);

  if (ttfts.length > 0) {
    const sorted = [...ttfts].sort((a, b) => a - b);
    console.log(`\n  TTFT (Time to First Token):`);
    console.log(`    Avg:  ${(ttfts.reduce((a, b) => a + b, 0) / ttfts.length).toFixed(1)}ms`);
    console.log(`    P50:  ${sorted[Math.floor(sorted.length * 0.5)].toFixed(1)}ms`);
    console.log(`    P95:  ${sorted[Math.floor(sorted.length * 0.95)].toFixed(1)}ms`);
  }

  if (interTokenGaps.length > 0) {
    const sorted = [...interTokenGaps].sort((a, b) => a - b);
    console.log(`  Inter-Token Gap:`);
    console.log(`    Avg:  ${(interTokenGaps.reduce((a, b) => a + b, 0) / interTokenGaps.length).toFixed(2)}ms`);
    console.log(`    Max:  ${sorted[sorted.length - 1].toFixed(2)}ms`);
  }
}

// ── E: Resilience Behavior ──
async function benchmarkResilience(agent) {
  console.log('\n' + '═'.repeat(60));
  console.log('  E — Resilience Behavior');
  console.log('═'.repeat(60));

  agent.resetPerfMetrics();

  let ok = 0;
  let fail = 0;
  for (let i = 0; i < ITERATIONS; i++) {
    try {
      await withTimeout(agent.runSimple('Say: ok'), 45000);
      ok++;
      process.stdout.write('.');
    } catch (e) {
      fail++;
      process.stdout.write('x');
    }
  }

  console.log(`  (${ok} ok, ${fail} failed)`);

  const m = agent.getPerfMetrics();
  console.log(`  Rate limit waits:    ${m.rateLimitWaits}`);
  console.log(`  Total wait time:     ${(m.totalRateLimitWaitUs / 1e3).toFixed(1)}ms`);
  console.log(`  Avg wait per wait:   ${m.rateLimitWaits > 0 ? (m.totalRateLimitWaitUs / m.rateLimitWaits / 1e3).toFixed(1) : 0}ms`);
  console.log(`  Circuit trips:       ${m.circuitTrips}`);
  console.log(`  LLM errors:          ${m.llmErrors}`);
  console.log(`  Error rate:          ${(m.llmErrors / ITERATIONS * 100).toFixed(1)}%`);
}

// ── Main ──
async function main() {
  console.log('\n' + '█'.repeat(60));
  console.log(`  BrainOS LLM Performance Harness v${version()}`);
  console.log(`  Model: ${MODEL}`);
  console.log(`  Iterations: ${ITERATIONS} (${QUICK ? 'quick' : 'full'} mode)`);
  console.log('█'.repeat(60));

  if (!API_KEY) {
    console.log('\n  ⚠️  No API key set. Set OPENAI_API_KEY environment variable.');
    process.exit(1);
  }

  const bus = await Bus.create();
  const results = {};

  // A: E2E Latency
  try {
    const agentA = await createAgent(bus);
    await warmup(agentA);
    results.e2e = await benchmarkE2E(agentA);
  } catch (e) { console.log(`  ⚠️  E2E test failed: ${e.message?.substring(0, 80)}`); }

  // B: Throughput
  try {
    const agentB = await createAgent(bus);
    await warmup(agentB);
    results.throughput = await benchmarkThroughput(agentB);
  } catch (e) { console.log(`  ⚠️  Throughput test failed: ${e.message?.substring(0, 80)}`); }

  // C: Overhead
  try {
    const agentC = await createAgent(bus);
    await warmup(agentC);
    results.overhead = await benchmarkOverhead(agentC);
  } catch (e) { console.log(`  ⚠️  Overhead test failed: ${e.message?.substring(0, 80)}`); }

  // D: Streaming
  try {
    const agentD = await createAgent(bus);
    await warmup(agentD);
    await benchmarkStreaming(agentD);
  } catch (e) { console.log(`  ⚠️  Streaming test failed: ${e.message?.substring(0, 80)}`); }

  // E: Resilience
  try {
    const agentE = await createAgent(bus);
    await benchmarkResilience(agentE);
  } catch (e) { console.log(`  ⚠️  Resilience test failed: ${e.message?.substring(0, 80)}`); }

  // Summary
  console.log('\n' + '═'.repeat(60));
  console.log('  Summary');
  console.log('═'.repeat(60));
  if (results.e2e) {
    console.log(`  E2E Avg Latency: ${results.e2e.avg.toFixed(0)}ms`);
  }
  if (results.overhead) {
    console.log(`  Engine vs Wall ratio: ${(results.overhead.totalEngineTimeUs / results.overhead.totalWallTimeUs * 100).toFixed(0)}%`);
  }
  console.log('');

  process.exit(0);
}

main().catch(err => {
  console.error('Fatal:', err.message || err);
  process.exit(1);
});