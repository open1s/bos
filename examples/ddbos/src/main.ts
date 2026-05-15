import { composeRoot } from './infrastructure/config/di.js';

async function main() {
  console.log('=== TRIZ Methodology Library with AI-Powered Domain Analysis ===\n');

  const deps = await composeRoot();

  console.log('1. Analyzing contradiction: Speed vs Force\n');
  const result = await deps.analyzeContradictionHandler.execute({
    improvingParameter: 9,
    worseningParameter: 10,
    description: 'Need higher processing speed but increased force causes wear',
    type: 'technical',
  });

  console.log('Contradiction ID:', result.contradictionId);
  console.log('Recommended Principles:');
  for (const p of result.recommendedPrinciples) {
    console.log(`  #${p.index}: ${p.name} - ${p.description}`);
  }

  console.log('\n2. Generating AI-enhanced solutions...\n');
  const solutions = await deps.generateSolutionsHandler.execute({
    contradictionId: result.contradictionId,
    aiEnhanced: false,
  });

  console.log('Generated Solutions:');
  for (const s of solutions.solutions) {
    console.log(`  [${s.principleName}] ${s.description}`);
  }

  console.log('\n3. Searching principles for "segmentation"...\n');
  const searchResults = deps.principleEngine.searchPrinciples('segmentation');
  for (const p of searchResults) {
    console.log(`  #${p.index}: ${p.name}`);
  }

  console.log('\n4. Evaluating ideality...\n');
  const ideality = await deps.idealityHandler.execute({
    problemId: 'prob_1',
    benefits: ['Faster processing', 'Lower latency', 'Better throughput'],
    costs: ['Higher energy consumption', 'More cooling needed'],
    harms: ['Increased wear on components'],
  });

  console.log('Ideality Score:', ideality.ideality.score);
  console.log('Level:', ideality.ideality.level);
  console.log('Recommendations:');
  for (const r of ideality.ideality.recommendations) {
    console.log(`  - ${r}`);
  }

  console.log('\n5. Available EZBOS Tools:');
  for (const tool of deps.tools) {
    console.log(`  - ${tool.name}: ${tool.description}`);
  }

  console.log('\n6. S-Curve Analysis: Battery Technology\n');
  const sCurveResult = await deps.sCurveHandler.execute({
    technologyName: 'Lithium-ion Batteries',
    performanceMetric: 'Wh/kg',
    dataPoints: [
      { x: 2010, y: 80 },
      { x: 2013, y: 120 },
      { x: 2016, y: 160 },
      { x: 2019, y: 200 },
      { x: 2022, y: 240 },
      { x: 2025, y: 265 },
    ],
  });

  console.log(sCurveResult.unicodeChart);
  console.log('\nS1 Stage:', sCurveResult.s1Stage);
  console.log('S2 Stage:', sCurveResult.s2Stage);
  console.log('Crossover Year:', sCurveResult.crossoverYear);
  if (sCurveResult.s1TRL) {
    console.log('S1 TRL:', sCurveResult.s1TRL.level, '-', sCurveResult.s1TRL.title);
  }
  if (sCurveResult.s2TRLRange) {
    console.log('S2 TRL Range:', sCurveResult.s2TRLRange.min, '-', sCurveResult.s2TRLRange.max);
  }
  console.log('\nRecommendations:');
  for (const r of sCurveResult.recommendations) {
    console.log(`  ${r}`);
  }

  console.log('\n=== TRIZ Library Ready ===');
  console.log('Use the tools with an EZBOS AgentBuilder:');
  console.log('  const agent = new AgentBuilder("triz-agent")');
  console.log('    .with_tools(...deps.tools)');
  console.log('    .start();');

  await deps.aiAgent.close();
  await deps.brain.stop();
}

main().catch(console.error);
