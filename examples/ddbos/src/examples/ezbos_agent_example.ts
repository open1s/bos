import { initTracing } from '@open1s/jsbos';
import { composeRoot } from '../infrastructure/config/di.js';

initTracing();

async function main() {
  console.log('=== TRIZ + EZBOS AI Agent (Real Search + TRL) ===\n');

  const deps = await composeRoot();

  const agent = deps.brain.agent('triz-expert')
    .with_systemPrompt(`You are a TRIZ (Theory of Inventive Problem Solving) expert with access to real-world research.

Available tools:
- triz_analyze_contradiction: Analyze technical contradictions
- triz_lookup_matrix: Look up the contradiction matrix
- triz_get_principle: Get details of a specific principle
- triz_search_principles: Search principles by keyword
- triz_list_principles: List all 40 principles
- triz_list_parameters: List all 39 engineering parameters
- triz_analyze_su_field: Analyze Substance-Field models
- triz_evaluate_ideality: Evaluate system ideality
- triz_ai_analyze: AI-powered contradiction analysis
- triz_ai_insight: AI-powered principle insights
- triz_trigger_search_patents: Search patents via OpenAlex (real data, free)
- triz_trigger_search_papers: Search papers via CrossRef/Semantic Scholar (real data, free)
- triz_trigger_search_prior_art: Search all sources (real data, free)
- triz_get_cached_patents: Get cached patent results
- triz_get_cached_papers: Get cached paper results
- triz_get_cached_prior_art: Get all cached results
- triz_list_cached_searches: List available cached searches
- triz_analyze_s_curve: Analyze technology S-curve with TRL assessment
- triz_extract_s_curve_data: Extract historical performance data for S-curve
- triz_enrich_s_curve: AI-refine S-curve parameters

All searches return real data from free APIs (OpenAlex, CrossRef, Semantic Scholar). No API keys required.`)
    .with_tools(...deps.tools);

  const started = await agent.start();

  console.log('Agent started with', started.tools.length, 'tools');
  console.log('\n--- User: 我想让我的车更快，但这样会消耗更多燃料. 使用TRIZ解决这个问题, 并搜索相关专利和论文. ---\n');

  let thinkingBuffer = '';
  let lastTokenType = '';

  await new Promise<void>((resolve) => {
    started.stream('我想让我的车更快，但这样会消耗更多燃料. 使用TRIZ解决这个问题, 并搜索相关专利和论文.', (token) => {
      if (token.type === 'ReasoningContent') {
        if (lastTokenType !== 'ReasoningContent') {
          if (thinkingBuffer) {
            console.log(`\n\x1b[90m💭 Thinking: ${thinkingBuffer}\x1b[0m\n`);
            thinkingBuffer = '';
          }
          process.stdout.write('\x1b[90m💭 Thinking: ');
        }
        process.stdout.write(token.text);
        thinkingBuffer += token.text;
      } else if (token.type === 'Text') {
        if (lastTokenType === 'ReasoningContent') {
          console.log(`\x1b[0m`);
          thinkingBuffer = '';
        }
        process.stdout.write(token.text);
      } else if (token.type === 'ToolCall') {
        if (lastTokenType === 'ReasoningContent') {
          console.log(`\x1b[0m`);
          thinkingBuffer = '';
        }
        console.log(`\n\x1b[36m🔧 Tool Call: ${token.name}\x1b[0m`);
      } else if (token.type === 'ToolResult') {
        console.log(`\x1b[32m✅ Tool Result received\x1b[0m`);
      } else if (token.type === 'Done') {
        if (thinkingBuffer) {
          console.log(`\n\x1b[90m💭 Thinking: ${thinkingBuffer}\x1b[0m`);
        }
        console.log('\n\n--- Done ---');
        resolve();
      } else if (token.type === 'Error') {
        console.log(`\n\x1b[31m❌ Error: ${token.error}\x1b[0m`);
        resolve();
      }

      lastTokenType = token.type;
    });
  });

  console.log('\n--- Metrics ---');
  console.log(JSON.stringify(started.metrics, null, 2));

  await started.close();
  await deps.brain.stop();
}

main().catch(console.error);
