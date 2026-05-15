import { composeRoot } from '../infrastructure/config/di.js';
import { writeFileSync, mkdirSync } from 'fs';

async function main() {
  console.log('=== TRIZ Unified Research API Demo ===\n');

  const deps = await composeRoot();

  // ─── Demo 1: Full research workflow ───
  console.log('1. Full Research: Electric Vehicle Battery\n');

  const result1 = await deps.unifiedResearch.research({
    problemDescription: 'How to increase electric vehicle battery capacity while reducing weight and cost?',
    improvingParameter: 'Weight of moving object',
    worseningParameter: 'Energy spent by moving object',
    technologyName: 'Lithium-ion Batteries',
    performanceMetric: 'Wh/kg',
    maxSearchResults: 3,
  });

  console.log(result1.summary);

  if (result1.contradictionAnalysis) {
    console.log('Recommended Principles:');
    for (const p of result1.contradictionAnalysis.principles.slice(0, 5)) {
      console.log(`  #${p.index} ${p.name}: ${p.description.slice(0, 80)}...`);
    }
    console.log();
  }

  console.log('Prior Art:');
  console.log(`  Patents: ${result1.priorArt.patents.length}`);
  for (const p of result1.priorArt.patents.slice(0, 2)) {
    console.log(`    - ${p.title.slice(0, 70)}...`);
  }
  console.log(`  Papers: ${result1.priorArt.papers.length}`);
  for (const p of result1.priorArt.papers.slice(0, 2)) {
    console.log(`    - ${p.title.slice(0, 70)}...`);
  }
  console.log(`  Tech Solutions: ${result1.priorArt.techSolutions.length}`);
  for (const t of result1.priorArt.techSolutions.slice(0, 2)) {
    console.log(`    - ${t.title.slice(0, 70)}...`);
  }
  console.log();

  if (result1.technologyMaturity) {
    const m = result1.technologyMaturity;
    console.log('Technology Maturity:');
    console.log(`  S-Curve: ${m.sCurveStage} → ${m.sCurveStageNext}`);
    console.log(`  TRL: ${m.trl.level}/9 - ${m.trl.title} (${Math.round(m.trl.confidence * 100)}%)`);
    console.log(`  Next-Gen TRL: ${m.trlNext.min}-${m.trlNext.max}/9`);
    console.log(`  Crossover: ~${m.crossoverYear}`);
    console.log();
  }

  console.log('Recommendations:');
  for (const r of result1.recommendations) {
    console.log(`  ✓ ${r}`);
  }

  // ─── Demo 2: Simple problem-only research ───
  console.log('\n\n2. Simple Research: Solar Panel Efficiency\n');

  const result2 = await deps.unifiedResearch.research({
    problemDescription: 'How to improve solar panel efficiency in low-light conditions?',
    searchQuery: 'solar panel efficiency low light photovoltaic',
    maxSearchResults: 2,
  });

  console.log(result2.summary);
  console.log('Prior Art Found:');
  console.log(`  Patents: ${result2.priorArt.patents.length}`);
  console.log(`  Papers: ${result2.priorArt.papers.length}`);
  console.log(`  Tech Solutions: ${result2.priorArt.techSolutions.length}`);
  console.log('\nRecommendations:');
  for (const r of result2.recommendations) {
    console.log(`  ✓ ${r}`);
  }

  // ─── Demo 3: API-style usage ───
  console.log('\n\n3. API-Style Usage: 5G Antenna Design\n');

  const researchAPI = {
    async solve(problem: string, options: {
      improving?: string;
      worsening?: string;
      technology?: string;
      metric?: string;
    } = {}) {
      return deps.unifiedResearch.research({
        problemDescription: problem,
        improvingParameter: options.improving,
        worseningParameter: options.worsening,
        technologyName: options.technology,
        performanceMetric: options.metric,
        maxSearchResults: 3,
      });
    },
  };

  const result3 = await researchAPI.solve(
    'How to make 5G antennas smaller while maintaining signal range?',
    {
      improving: 'Size of moving object',
      worsening: 'Loss of information',
      technology: '5G Antennas',
      metric: 'dBi gain',
    },
  );

  console.log(result3.summary);
  console.log('Recommendations:');
  for (const r of result3.recommendations) {
    console.log(`  ✓ ${r}`);
  }

  await deps.aiAgent.close();
  await deps.aiSCurveDataExtractor.close();
  await deps.trlAssessor.close();
  await deps.brain.stop();

  console.log('\n=== Demo Complete ===');
  console.log('\nUsage:');
  console.log('  import { composeRoot } from "./infrastructure/config/di.js";');
  console.log('  const deps = await composeRoot();');
  console.log('  const result = await deps.unifiedResearch.research({');
  console.log('    problemDescription: "Your problem here",');
  console.log('    improvingParameter: "Parameter to improve",');
  console.log('    worseningParameter: "Parameter that worsens",');
  console.log('    technologyName: "Technology name",');
  console.log('    performanceMetric: "Metric (e.g., Wh/kg)",');
  console.log('    maxSearchResults: 5,');
  console.log('  });');
}

main().catch(console.error);
