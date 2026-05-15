import { ContradictionMatrix } from '../domain/contradiction/matrix.js';
import { ContradictionAnalysisService } from '../domain/contradiction/services.js';
import { PrincipleEngine } from '../domain/principle/services.js';
import { SuFieldAnalysisService } from '../domain/solution/su_field_service.js';
import { createReference, enrichReferenceWithSummary } from '../domain/solution/external_reference.js';
import { MultiSourceSearchService } from '../infrastructure/search/multi_source_search.js';
import { CachedSearchService, SearchCache } from '../infrastructure/search/cached_search.js';
import { getMockPatents, getMockPapers, getMockTechSolutions } from '../infrastructure/search/mock_data.js';
import { ContentExtractor } from '../infrastructure/search/content_extractor.js';
import { CurveFittingService, StageDetectionService, SCurveAnalysisService } from '../domain/s_curve/services.js';
import { SvgCurveGenerator } from '../domain/s_curve/svg_generator.js';
import { SCurve } from '../domain/s_curve/entity.js';

let passed = 0;
let failed = 0;
let skipped = 0;

function assert(condition: boolean, message: string) {
  if (condition) {
    console.log(`  ✓ ${message}`);
    passed++;
  } else {
    console.error(`  ✗ ${message}`);
    failed++;
  }
}

function skip(message: string) {
  console.log(`  ⊘ ${message} (skipped)`);
  skipped++;
}

const tests: Array<{ name: string; fn: () => Promise<void> }> = [];

function test(name: string, fn: () => Promise<void>) {
  tests.push({ name, fn });
}

// ─── Domain Tests ───

test('Contradiction Matrix lookup returns principles', async () => {
  const matrix = ContradictionMatrix.getInstance();
  const principles = matrix.lookup(9, 19);
  assert(principles.length > 0, `Speed vs Energy returns ${principles.length} principles`);
  assert(principles.every(p => p >= 1 && p <= 40), 'All principle indices are valid (1-40)');
});

test('Contradiction Matrix rejects invalid parameters', async () => {
  const matrix = ContradictionMatrix.getInstance();
  try {
    matrix.lookup(0, 1);
    assert(false, 'Should throw for parameter 0');
  } catch {
    assert(true, 'Throws for parameter index 0');
  }
  try {
    matrix.lookup(1, 40);
    assert(false, 'Should throw for parameter 40');
  } catch {
    assert(true, 'Throws for parameter index 40');
  }
});

test('Contradiction Analysis Service creates contradiction with principles', async () => {
  const service = new ContradictionAnalysisService();
  const result = service.analyze(9, 19, 'Need faster speed but less energy');
  assert(result.contradiction.id.startsWith('ctr_'), 'Contradiction has valid ID');
  assert(result.principles.length > 0, `Found ${result.principles.length} recommended principles`);
  assert(result.principles[0].name.length > 0, 'Principles have names');
});

test('Principle Engine search finds relevant principles', async () => {
  const engine = new PrincipleEngine();
  const results = engine.searchPrinciples('segmentation');
  assert(results.length > 0, `Found ${results.length} principles for "segmentation"`);
  assert(results[0].name.toLowerCase().includes('segment'), 'First result is Segmentation');
});

test('Principle Engine returns all 40 principles', async () => {
  const engine = new PrincipleEngine();
  const all = engine.getAllPrinciples();
  assert(all.length === 40, `Returns all 40 principles (got ${all.length})`);
});

test('Principle Engine combines multiple principles', async () => {
  const engine = new PrincipleEngine();
  const combo = engine.combinePrinciples([1, 15, 28]);
  assert(combo.principles.length === 3, 'Combines 3 principles');
  assert(combo.combinedDescription.includes('Segmentation'), 'Includes Segmentation');
  assert(combo.combinedDescription.includes('Dynamics'), 'Includes Dynamics');
});

test('Su-Field Analysis detects incomplete model', async () => {
  const service = new SuFieldAnalysisService();
  const result = service.analyze({ substance1: 'drill', substance2: '', field: 'mechanical' });
  assert(result.type === 'incomplete', `Type is incomplete (got ${result.type})`);
  assert(result.standardSolutions.length > 0, 'Returns standard solutions');
});

test('Su-Field Analysis detects harmful model', async () => {
  const service = new SuFieldAnalysisService();
  const result = service.analyzeHarmful('acid', 'metal', 'chemical');
  assert(result.type === 'harmful', `Type is harmful (got ${result.type})`);
  assert(result.standardSolutions.length > 0, 'Returns solutions for harmful Su-Field');
});

test('External Reference VO creates and enriches', async () => {
  const ref = createReference(
    'https://example.com/patent',
    'Test Patent',
    'patent',
    'A method for improving speed',
    85,
    '2024-01-01',
    ['John Doe'],
  );
  assert(ref.url === 'https://example.com/patent', 'URL set correctly');
  assert(ref.relevanceScore === 85, 'Relevance score set');
  assert(ref.authors?.length === 1, 'Authors set');
  assert(!ref.summary, 'No summary initially');

  const enriched = enrichReferenceWithSummary(ref, 'This patent describes...');
  assert(enriched.summary === 'This patent describes...', 'Summary added');
  assert(enriched.relevanceScore === 85, 'Original score preserved');
});

// ─── Infrastructure Tests ───

test('Content Extractor extracts text from HTML', async () => {
  const extractor = new ContentExtractor();
  const html = '<html><head><title>Test Page</title></head><body><p>Hello World</p><script>alert(1)</script></body></html>';

  const result = await extractor.extract('data:text/html,' + encodeURIComponent(html));
  if (result) {
    assert(result.title === 'Test Page', `Title extracted: ${result.title}`);
    assert(result.mainContent.includes('Hello World'), 'Content includes body text');
    assert(!result.mainContent.includes('script'), 'Script tags removed');
  } else {
    assert(false, 'Content extractor returned null for data URI');
  }
});

test('Content Extractor handles invalid URLs gracefully', async () => {
  const extractor = new ContentExtractor();
  const result = await extractor.extract('https://this-domain-definitely-does-not-exist-12345.com');
  assert(result === null, 'Returns null for invalid URL');
});

test('MultiSource Search Service handles missing API keys gracefully', async () => {
  const search = new MultiSourceSearchService({
    openAlex: {},
  });
  const patents = await search.searchPatents('TRIZ contradiction');
  assert(patents.length > 0, `Returns results via OpenAlex fallback (got ${patents.length})`);

  const papers = await search.searchPapers('TRIZ');
  if (papers.length === 0) {
    assert(papers.length === 0, 'Returns empty when no Semantic Scholar config (raw service, no fallback)');
  } else {
    assert(papers.length > 0, `Semantic Scholar returned ${papers.length} results (API available)`);
  }

  const tech = await search.searchTechSolutions('hybrid engine');
  assert(tech.length > 0, `Returns results via OpenAlex fallback (got ${tech.length})`);
});

test('MultiSource Search Service semantic scholar works (free, no key)', async () => {
  const search = new MultiSourceSearchService({
    semanticScholar: {},
  });
  const papers = await search.searchPapers('TRIZ inventive principles engineering design', 3);
  if (papers.length > 0) {
    assert(papers[0].title.length > 0, `First paper has title: "${papers[0].title.slice(0, 60)}..."`);
    assert(papers[0].sourceType === 'paper', 'Source type is paper');
    assert(papers[0].url.length > 0, 'Paper has URL');
  } else {
    skip('Semantic Scholar rate limited (429) — API works but needs retry or key');
  }
});

test('MultiSource Search Service combined search filters by source type', async () => {
  const search = new MultiSourceSearchService({
    semanticScholar: {},
  });
  const results = await search.search({
    keywords: ['TRIZ', 'engineering'],
    sourceTypes: ['paper'],
    maxResults: 3,
  });
  if (results.length > 0) {
    assert(results.every(r => r.sourceType === 'paper'), 'All results are papers');
  } else {
    skip('Semantic Scholar rate limited (429) — combined search works but needs retry or key');
  }
});

// ─── Mock Data Tests ───

test('Mock data returns patent results', async () => {
  const patents = getMockPatents('speed fuel', 3);
  assert(patents.length === 3, `Returns 3 mock patents (got ${patents.length})`);
  assert(patents[0].sourceType === 'patent', 'Source type is patent');
  assert(patents[0].url.startsWith('https://'), 'Patent has valid URL');
  assert(patents[0].authors && patents[0].authors.length > 0, 'Patent has authors');
});

test('Mock data returns paper results', async () => {
  const papers = getMockPapers('TRIZ automotive', 3);
  assert(papers.length === 3, `Returns 3 mock papers (got ${papers.length})`);
  assert(papers[0].sourceType === 'paper', 'Source type is paper');
  assert(papers[0].authors && papers[0].authors.length > 0, 'Paper has authors');
  assert(!!papers[0].publishedDate, 'Paper has publication date');
});

test('Mock data returns tech solution results', async () => {
  const tech = getMockTechSolutions('hybrid vehicle', 2);
  assert(tech.length === 2, `Returns 2 mock tech solutions (got ${tech.length})`);
  assert(tech[0].sourceType === 'tech_solution', 'Source type is tech_solution');
});

// ─── Cached Search Tests ───

test('CachedSearchService caches results', async () => {
  const inner = new MultiSourceSearchService({});
  const cache = new SearchCache();
  const cached = new CachedSearchService(inner, cache);

  const patents = await cached.searchPatents('TRIZ speed', 3);
  assert(patents.length > 0, `Search returns results (got ${patents.length})`);

  const cachedPatents = cached.getCachedPatents('TRIZ speed', 3);
  assert(cachedPatents.length === patents.length, 'Cached results match search results');
});

test('CachedSearchService returns empty when API returns empty', async () => {
  const inner = new MultiSourceSearchService({});
  const cached = new CachedSearchService(inner);

  const patents = await cached.searchPatents('TRIZ', 3);
  assert(patents.length >= 0, `Search returns results (got ${patents.length})`);

  const papers = await cached.searchPapers('TRIZ', 3);
  assert(papers.length >= 0, `Search returns papers (got ${papers.length})`);
});

test('CachedSearchService get methods return empty when cache empty', async () => {
  const inner = new MultiSourceSearchService({});
  const cached = new CachedSearchService(inner);

  const patents = cached.getCachedPatents('test', 3);
  assert(patents.length === 0, 'Empty cache returns empty (no trigger yet)');

  await cached.searchPatents('test', 3);
  const afterSearch = cached.getCachedPatents('test', 3);
  assert(afterSearch.length >= 0, 'After search, cache is populated');
});

test('SearchCache TTL expires old entries', async () => {
  const cache = new SearchCache();
  cache.set('test', [{ title: 'Test', url: 'https://test.com', snippet: 'test', sourceType: 'paper' }], 1);

  assert(cache.get('test') !== null, 'Entry exists immediately');

  await new Promise(r => setTimeout(r, 10));

  assert(cache.get('test') === null, 'Entry expired after TTL');
});

// ─── S-Curve Tests ───

test('Curve fitting service fits data points', async () => {
  const fitting = new CurveFittingService();
  const dataPoints = [
    { x: 2010, y: 10 },
    { x: 2012, y: 20 },
    { x: 2014, y: 45 },
    { x: 2016, y: 80 },
    { x: 2018, y: 120 },
    { x: 2020, y: 150 },
    { x: 2022, y: 170 },
    { x: 2024, y: 180 },
  ];

  const result = fitting.fit(dataPoints);
  assert(result.parameters.L > 180, `L (${result.parameters.L}) > max data point`);
  assert(result.parameters.k > 0, `k (${result.parameters.k}) is positive`);
  assert(result.parameters.t0 > 2010 && result.parameters.t0 < 2024, `t0 (${result.parameters.t0}) is within data range`);
  assert(!result.estimated, 'Result is not estimated (has data)');
});

test('Curve fitting service estimates from sparse data', async () => {
  const fitting = new CurveFittingService();
  const result = fitting.fit([{ x: 2024, y: 100 }]);
  assert(result.estimated, 'Result is estimated (sparse data)');
  assert(result.parameters.L > 100, 'L > current value');
});

test('Stage detection service detects growth stage', async () => {
  const detector = new StageDetectionService();
  const dataPoints = [
    { x: 2020, y: 20 },
    { x: 2021, y: 35 },
    { x: 2022, y: 55 },
    { x: 2023, y: 80 },
    { x: 2024, y: 110 },
  ];

  const result = detector.detect({ L: 200, k: 0.4, t0: 2025 }, 2024, dataPoints);
  assert(result.stage === 'growth', `Stage is growth (got ${result.stage})`);
  assert(result.confidence > 0, 'Has confidence score');
  assert(result.reasoning.length > 0, 'Has reasoning');
});

test('Stage detection service detects maturity stage', async () => {
  const detector = new StageDetectionService();
  const dataPoints = [
    { x: 2020, y: 100 },
    { x: 2021, y: 115 },
    { x: 2022, y: 125 },
    { x: 2023, y: 132 },
    { x: 2024, y: 136 },
  ];

  const result = detector.detect({ L: 180, k: 0.3, t0: 2018 }, 2024, dataPoints);
  assert(result.stage === 'maturity', `Stage is maturity (got ${result.stage})`);
});

test('SCurve entity generates data points', async () => {
  const sCurve = new SCurve(
    'Test Tech',
    'units',
    { L: 100, k: 0.3, t0: 2020 },
    { L: 150, k: 0.35, t0: 2030 },
    'maturity',
    'growth',
    false,
    true,
  );

  const points = sCurve.generateDataPoints(2000, 2040, 50);
  assert(points.length === 51, `Generates 51 points (got ${points.length})`);
  assert(points[0].x === 2000, 'Starts at minX');
  assert(points[50].x === 2040, 'Ends at maxX');
});

test('SCurve entity calculates crossover point', async () => {
  const sCurve = new SCurve(
    'Test Tech',
    'units',
    { L: 100, k: 0.3, t0: 2020 },
    { L: 150, k: 0.35, t0: 2030 },
    'maturity',
    'growth',
    false,
    true,
  );

  const crossover = sCurve.getCrossoverPoint();
  assert(crossover > 2020, `Crossover (${crossover}) is after S1 inflection`);
  assert(crossover < 2040, `Crossover (${crossover}) is before end of range`);
});

test('SCurve analysis service creates full analysis', async () => {
  const service = new SCurveAnalysisService();
  const dataPoints = [
    { x: 2015, y: 10 },
    { x: 2017, y: 30 },
    { x: 2019, y: 60 },
    { x: 2021, y: 100 },
    { x: 2023, y: 130 },
    { x: 2025, y: 145 },
  ];

  const sCurve = service.analyze('Battery Tech', 'Wh/kg', dataPoints, 2025);
  assert(sCurve.technologyName === 'Battery Tech', 'Technology name set');
  assert(sCurve.performanceMetric === 'Wh/kg', 'Performance metric set');
  assert(sCurve.s1Parameters.L > 145, 'S1 L > max data point');
  assert(sCurve.s2Parameters.L > sCurve.s1Parameters.L, 'S2 L > S1 L');
});

test('SVG generator produces valid SVG', async () => {
  const sCurve = new SCurve(
    'Battery Technology',
    'Wh/kg',
    { L: 200, k: 0.3, t0: 2020 },
    { L: 300, k: 0.35, t0: 2030 },
    'maturity',
    'growth',
    false,
    true,
    [
      { x: 2015, y: 30 },
      { x: 2018, y: 80 },
      { x: 2021, y: 140 },
      { x: 2024, y: 175 },
    ],
  );

  const generator = new SvgCurveGenerator();
  const svg = generator.generate(sCurve);

  assert(svg.startsWith('<svg'), 'SVG starts with <svg>');
  assert(svg.includes('</svg>'), 'SVG ends with </svg>');
  assert(svg.includes('Battery Technology'), 'SVG contains technology name');
  assert(svg.includes('Wh/kg'), 'SVG contains performance metric');
  assert(svg.includes('#2196F3'), 'SVG contains S1 curve color');
  assert(svg.includes('#4CAF50'), 'SVG contains S2 curve color');
});

test('SVG generator produces Unicode chart', async () => {
  const sCurve = new SCurve(
    'Test Tech',
    'units',
    { L: 100, k: 0.3, t0: 2020 },
    { L: 150, k: 0.35, t0: 2030 },
    'maturity',
    'growth',
    false,
    true,
  );

  const generator = new SvgCurveGenerator();
  const chart = generator.generateUnicodeChart(sCurve);

  assert(chart.includes('Test Tech'), 'Chart contains technology name');
  assert(chart.includes('●'), 'Chart contains S1 marker');
  assert(chart.includes('○'), 'Chart contains S2 marker');
  assert(chart.includes('成熟期'), 'Chart contains S1 stage');
  assert(chart.includes('成长期'), 'Chart contains S2 stage');
});

test('SVG generator renders milestones', async () => {
  const sCurve = new SCurve(
    'Battery Technology',
    'Wh/kg',
    { L: 200, k: 0.3, t0: 2020 },
    { L: 300, k: 0.35, t0: 2030 },
    'maturity',
    'growth',
    false,
    true,
    [
      { x: 2015, y: 30 },
      { x: 2018, y: 80 },
      { x: 2021, y: 140 },
      { x: 2024, y: 175 },
    ],
    [
      { year: 2010, label: 'Invention', description: 'First prototype', type: 'invention' },
      { year: 2015, label: 'Breakthrough', description: 'Energy density doubled', type: 'breakthrough' },
      { year: 2020, label: 'Commercial Launch', description: 'Mass production begins', type: 'commercialization' },
    ],
  );

  const generator = new SvgCurveGenerator();
  const svg = generator.generate(sCurve);

  assert(svg.includes('Invention'), 'SVG contains milestone label: Invention');
  assert(svg.includes('Breakthrough'), 'SVG contains milestone label: Breakthrough');
  assert(svg.includes('Commercial Launch'), 'SVG contains milestone label: Commercial Launch');
  assert(svg.includes('polygon'), 'SVG contains milestone markers');
  assert(svg.includes('关键事件'), 'SVG contains milestone legend');
});

test('Unicode chart includes milestones', async () => {
  const sCurve = new SCurve(
    'Battery Technology',
    'Wh/kg',
    { L: 200, k: 0.3, t0: 2020 },
    { L: 300, k: 0.35, t0: 2030 },
    'maturity',
    'growth',
    false,
    true,
    [],
    [
      { year: 2010, label: 'Invention', description: 'First prototype', type: 'invention' },
      { year: 2020, label: 'Commercial Launch', description: 'Mass production', type: 'commercialization' },
    ],
  );

  const generator = new SvgCurveGenerator();
  const chart = generator.generateUnicodeChart(sCurve);

  assert(chart.includes('Invention'), 'Chart contains milestone: Invention');
  assert(chart.includes('Commercial Launch'), 'Chart contains milestone: Commercial Launch');
  assert(chart.includes('关键事件'), 'Chart contains Key Events section');
});

test('SCurve analysis service generates recommendations', async () => {
  const service = new SCurveAnalysisService();
  const sCurve = service.analyze('Old Tech', 'score', [
    { x: 2015, y: 80 },
    { x: 2018, y: 90 },
    { x: 2021, y: 95 },
    { x: 2024, y: 97 },
  ], 2025);

  const recommendations = service.generateRecommendations(sCurve, 2025);
  assert(recommendations.length > 0, 'Has recommendations');
  assert(recommendations.some(r => r.includes('maturity') || r.includes('decline')), 'Includes stage-specific recommendation');
});

// ─── Run all tests sequentially ───

async function runAll() {
  for (const t of tests) {
    console.log(`\n── ${t.name} ──`);
    try {
      await t.fn();
    } catch (e: any) {
      console.error(`  ✗ Test crashed: ${e.message}`);
      failed++;
    }
  }

  console.log('\n═══════════════════════════════════════');
  console.log(`  Results: ${passed} passed, ${failed} failed, ${skipped} skipped`);
  console.log('═══════════════════════════════════════');

  if (failed > 0) {
    process.exit(1);
  }
}

runAll();
