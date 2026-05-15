import { composeRoot } from '../infrastructure/config/di.js';
import { writeFileSync, mkdirSync } from 'fs';

async function main() {
  console.log('=== TRIZ TRL Assessment Demo ===\n');

  const deps = await composeRoot();

  // ─── Demo 1: AI-extracted data + TRL assessment ───
  console.log('1. Lithium-ion Batteries - AI Data Extraction + TRL Assessment\n');

  const extracted1 = await deps.aiSCurveDataExtractor.extractData(
    'Lithium-ion Batteries',
    'Wh/kg',
  );

  console.log(`Extracted ${extracted1.dataPoints.length} data points`);
  console.log('Data:', JSON.stringify(extracted1.dataPoints.slice(0, 3), null, 2));
  console.log('...\n');

  if (extracted1.dataPoints.length > 0) {
    const result1 = await deps.sCurveHandler.execute({
      technologyName: 'Lithium-ion Batteries',
      performanceMetric: 'Wh/kg',
      dataPoints: extracted1.dataPoints,
    });

    console.log('S-Curve Stage:', result1.s1Stage, '→', result1.s2Stage);
    console.log('Crossover Year:', result1.crossoverYear);
    console.log('\nTRL Assessment:');
    if (result1.s1TRL) {
      console.log(`  S1: TRL ${result1.s1TRL.level}/9 - ${result1.s1TRL.title}`);
      console.log(`      Confidence: ${Math.round(result1.s1TRL.confidence * 100)}%`);
      console.log(`      Evidence: ${result1.s1TRL.evidence.length} sources`);
    }
    if (result1.s2TRLRange) {
      console.log(`  S2: TRL ${result1.s2TRLRange.min}-${result1.s2TRLRange.max}/9 (most likely: ${result1.s2TRLRange.mostLikely})`);
    }
    console.log('\nTRL Reconciliation:');
    console.log(`  ${result1.trlReconciliation?.slice(0, 200)}...\n`);

    mkdirSync('output', { recursive: true });
    writeFileSync('output/battery-scurve.svg', result1.svg);
    console.log('✅ SVG saved to output/battery-scurve.svg\n');
  }

  // ─── Demo 2: User-provided TRL override ───
  console.log('2. Solid State Batteries - User TRL Override\n');

  const extracted2 = await deps.aiSCurveDataExtractor.extractData(
    'Solid State Batteries',
    'Wh/kg',
  );

  console.log(`Extracted ${extracted2.dataPoints.length} data points`);

  if (extracted2.dataPoints.length > 0) {
    const result2 = await deps.sCurveHandler.execute({
      technologyName: 'Solid State Batteries',
      performanceMetric: 'Wh/kg',
      dataPoints: extracted2.dataPoints,
      trl: 6,
      trlReasoning: 'Toyota announced prototype solid-state battery EV for 2027-2028, indicating TRL 6-7 level development',
    });

    console.log('\nS-Curve Stage:', result2.s1Stage, '→', result2.s2Stage);
    if (result2.s1TRL) {
      console.log(`S1 TRL: ${result2.s1TRL.level}/9 - ${result2.s1TRL.title}`);
      console.log(`      User-provided: ${result2.s1TRL.isUserProvided}`);
      console.log(`      Confidence: ${Math.round(result2.s1TRL.confidence * 100)}%`);
    }
    if (result2.s2TRLRange) {
      console.log(`S2 TRL Range: ${result2.s2TRLRange.min}-${result2.s2TRLRange.max}/9`);
    }
    console.log('\nReconciliation:');
    console.log(`  ${result2.trlReconciliation?.slice(0, 200)}...\n`);

    writeFileSync('output/solid-state-scurve.svg', result2.svg);
    console.log('✅ SVG saved to output/solid-state-scurve.svg\n');
  }

  // ─── Demo 3: Internal Combustion Engines ───
  console.log('3. Internal Combustion Engines - Mature Technology\n');

  const extracted3 = await deps.aiSCurveDataExtractor.extractData(
    'Internal Combustion Engines',
    'HP/liter',
  );

  console.log(`Extracted ${extracted3.dataPoints.length} data points`);

  if (extracted3.dataPoints.length > 0) {
    const result3 = await deps.sCurveHandler.execute({
      technologyName: 'Internal Combustion Engines',
      performanceMetric: 'HP/liter',
      dataPoints: extracted3.dataPoints,
    });

    console.log('\nS-Curve Stage:', result3.s1Stage, '→', result3.s2Stage);
    if (result3.s1TRL) {
      console.log(`S1 TRL: ${result3.s1TRL.level}/9 - ${result3.s1TRL.title}`);
      console.log(`      Confidence: ${Math.round(result3.s1TRL.confidence * 100)}%`);
    }
    if (result3.s2TRLRange) {
      console.log(`S2 TRL Range: ${result3.s2TRLRange.min}-${result3.s2TRLRange.max}/9`);
    }
    console.log('\nUnicode Chart:');
    console.log(result3.unicodeChart);
  }

  await deps.aiAgent.close();
  await deps.aiSCurveDataExtractor.close();
  await deps.trlAssessor.close();
  await deps.brain.stop();

  console.log('\n=== Demo Complete ===');
  console.log('Open the .svg files in a browser to see the S-curves with TRL badges!');
}

main().catch(console.error);
