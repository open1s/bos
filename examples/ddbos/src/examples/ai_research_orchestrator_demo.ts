import { composeRoot } from '../infrastructure/config/di.js';

function thinking(step: string, message: string) {
  const icons: Record<string, string> = {
    search: '🔍',
    summarize: '📝',
    analyze: '🧠',
    triz: '⚙️',
    thinking: '💭',
    tool: '🔧',
    keywords: '🔑',
    contradiction: '⚖️',
    s_curve: '📈',
  };
  const icon = icons[step] || '•';
  console.log(`  ${icon} [${step.toUpperCase()}] ${message}`);
}

async function main() {
  console.log('=== AI Research Orchestrator Demo ===\n');
  console.log('AI searches, summarizes, and generates research report.\n');
  console.log('New features:');
  console.log('  - Streaming AI calls with thinking output');
  console.log('  - Parallel AI summarization (faster)');
  console.log('  - Relevance scoring & ranking');
  console.log('  - Error tracking & warnings');
  console.log('  - Research metadata (duration, sources, AI calls)\n');

  const deps = await composeRoot();

  // ─── Demo 1: Simple problem, AI does everything ───
  console.log('1. AI-Driven Research: Electric Vehicle Battery\n');
  console.log('Input: "我想让电动车电池续航更长，但这样会增加电池重量和成本"');
  console.log('');

  let thinkingOutput = '';
  const result1 = await deps.aiResearchOrchestrator.research(
    '我想让电动车电池续航更长，但这样会增加电池重量和成本。 用中文回答',
    {
      maxSearchResults: 2,
      onProgress: thinking,
      showThinking: true,
      onThinking: (text) => {
        thinkingOutput += text;
        // Print thinking in real-time with prefix
        const lines = text.split('\n');
        for (const line of lines) {
          if (line.trim()) {
            process.stdout.write(`    💭 ${line}\n`);
          }
        }
      },
    },
  );

  console.log('\n' + result1.summary);

  // Show metadata
  if (result1.metadata) {
    console.log('---');
    console.log(`Duration: ${Math.round(result1.metadata.durationMs / 1000)}s`);
    console.log(`Sources: ${result1.metadata.sourcesUsed.join(', ')}`);
    console.log(`AI calls: ${result1.metadata.aiCallsMade}`);
    console.log(`Errors: ${result1.errors.length}`);
  }

  // ─── Demo 2: 5G Antenna ───
  console.log('\n\n2. AI-Driven Research: 5G Antenna Design\n');
  console.log('Input: "开发一个永动机玩具,用中文回答"');
  console.log('');

  const result2 = await deps.aiResearchOrchestrator.research(
    '开发一个永动机玩具',
    {
      maxSearchResults: 2,
      onProgress: thinking,
      showThinking: true,
      onThinking: (text) => {
        const lines = text.split('\n');
        for (const line of lines) {
          if (line.trim()) {
            process.stdout.write(`    💭 ${line}\n`);
          }
        }
      },
    },
  );

  console.log('\n' + result2.summary);

  // Show relevance-sorted results
  console.log('---');
  console.log('Top patents by relevance:');
  for (const p of result2.priorArt.patents.slice(0, 3)) {
    console.log(`  [${p.relevanceScore ?? 'N/A'}] ${p.title}`);
  }

  await deps.aiAgent.close();
  await deps.aiResearchOrchestrator.close();
  await deps.brain.stop();

  console.log('\n=== Demo Complete ===');
  console.log('\nUsage:');
  console.log('  import { composeRoot } from "./infrastructure/config/di.js";');
  console.log('  const deps = await composeRoot();');
  console.log('  const result = await deps.aiResearchOrchestrator.research(');
  console.log('    "Your problem description here",');
  console.log('    {');
  console.log('      maxSearchResults: 5,');
  console.log('      showThinking: true,');
  console.log('      onProgress: (step, msg) => console.log(`[${step}] ${msg}`),');
  console.log('      onThinking: (text) => process.stdout.write(text)');
  console.log('    }');
  console.log('  );');
  console.log('  console.log(result.summary);');
  console.log('  console.log(result.errors);     // Track warnings/errors');
  console.log('  console.log(result.metadata);   // Duration, sources, AI calls');
  console.log('  console.log(result.priorArt.patents[0].relevanceScore);');
}

main().catch(console.error);
