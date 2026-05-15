import { composeRoot } from '../infrastructure/config/di.js';
import { writeFileSync, mkdirSync } from 'fs';

async function main() {
  console.log('=== TRIZ S-Curve Demo (AI Data Extraction) ===\n');

  const deps = await composeRoot();

  // ─── Demo 1: AI-extracted data (Lithium-ion Batteries) ───
  console.log('1. Extracting data via AI search (Lithium-ion Batteries)\n');

  const extracted1 = await deps.aiSCurveDataExtractor.extractData(
    'Lithium-ion Batteries',
    'Wh/kg',
  );

  console.log(`Extracted ${extracted1.dataPoints.length} data points:`);
  console.log(JSON.stringify(extracted1.dataPoints, null, 2));
  console.log('\nSources:', extracted1.sources.slice(0, 3));
  console.log('\nReasoning:', extracted1.reasoning);

  if (extracted1.dataPoints.length > 0) {
    const result1 = await deps.sCurveHandler.execute({
      technologyName: 'Lithium-ion Batteries',
      performanceMetric: 'Wh/kg',
      dataPoints: extracted1.dataPoints,
    });

    console.log('\n' + result1.unicodeChart);
    console.log('S1 Stage:', result1.s1Stage);
    console.log('S2 Stage:', result1.s2Stage);
    console.log('Crossover:', result1.crossoverYear);
    if (result1.s1TRL) {
      console.log('S1 TRL:', result1.s1TRL.level, '-', result1.s1TRL.title);
    }
    if (result1.s2TRLRange) {
      console.log('S2 TRL Range:', result1.s2TRLRange.min, '-', result1.s2TRLRange.max);
    }
    console.log('\nAnalysis:\n', result1.analysis);

    mkdirSync('output', { recursive: true });
    writeFileSync('output/battery-scurve.svg', result1.svg);
    console.log('\n✅ SVG saved to output/battery-scurve.svg\n');
  }

  // ─── Demo 2: AI-extracted data (Solid State Batteries) ───
  console.log('2. Extracting data via AI search (Solid State Batteries)\n');

  const extracted2 = await deps.aiSCurveDataExtractor.extractData(
    'Solid State Batteries',
    'Wh/kg',
  );

  console.log(`Extracted ${extracted2.dataPoints.length} data points:`);
  console.log(JSON.stringify(extracted2.dataPoints, null, 2));

  if (extracted2.dataPoints.length > 0) {
    const result2 = await deps.sCurveHandler.execute({
      technologyName: 'Solid State Batteries',
      performanceMetric: 'Wh/kg',
      dataPoints: extracted2.dataPoints,
    });

    console.log('\n' + result2.unicodeChart);
    console.log('S1 Stage:', result2.s1Stage);
    console.log('S2 Stage:', result2.s2Stage);
    console.log('Crossover:', result2.crossoverYear);
    if (result2.s1TRL) {
      console.log('S1 TRL:', result2.s1TRL.level, '-', result2.s1TRL.title);
    }
    if (result2.s2TRLRange) {
      console.log('S2 TRL Range:', result2.s2TRLRange.min, '-', result2.s2TRLRange.max);
    }

    writeFileSync('output/solid-state-scurve.svg', result2.svg);
    console.log('\n✅ SVG saved to output/solid-state-scurve.svg\n');
  }

  // ─── Demo 3: AI-extracted data (Internal Combustion Engines) ───
  console.log('3. Extracting data via AI search (Internal Combustion Engines)\n');

  const extracted3 = await deps.aiSCurveDataExtractor.extractData(
    'Internal Combustion Engines',
    'HP/liter',
  );

  console.log(`Extracted ${extracted3.dataPoints.length} data points:`);
  console.log(JSON.stringify(extracted3.dataPoints, null, 2));

  if (extracted3.dataPoints.length > 0) {
    const result3 = await deps.sCurveHandler.execute({
      technologyName: 'Internal Combustion Engines',
      performanceMetric: 'HP/liter',
      dataPoints: extracted3.dataPoints,
    });

    console.log('\n' + result3.unicodeChart);
    console.log('S1 Stage:', result3.s1Stage);
    console.log('S2 Stage:', result3.s2Stage);
    console.log('Crossover:', result3.crossoverYear);
    if (result3.s1TRL) {
      console.log('S1 TRL:', result3.s1TRL.level, '-', result3.s1TRL.title);
    }
    if (result3.s2TRLRange) {
      console.log('S2 TRL Range:', result3.s2TRLRange.min, '-', result3.s2TRLRange.max);
    }
  }

  await deps.aiAgent.close();
  await deps.aiSCurveDataExtractor.close();
  await deps.brain.stop();

  console.log('\n=== Demo Complete ===');
  console.log('Open the .svg files in a browser to see the beautiful S-curves!');
}

main().catch(console.error);
