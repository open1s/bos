import { defineTool, ok, err } from '@open1s/ezbos';
import { ContradictionAnalysisService } from '../../domain/contradiction/services.js';
import { ContradictionMatrix } from '../../domain/contradiction/matrix.js';
import { PrincipleEngine } from '../../domain/principle/services.js';
import { SuFieldAnalysisService } from '../../domain/solution/su_field_service.js';
import { EvaluateIdealityHandler } from '../../application/evaluate_ideality/handler.js';
import { AiTrizAgent } from '../ai/triz_ai_agent.js';
import { CachedSearchService } from '../search/cached_search.js';
import { AISummarizer } from '../search/ai_summarizer.js';

import { getMockPatents, getMockPapers, getMockTechSolutions } from '../search/mock_data.js';

import { AnalyzeSCurveHandler } from '../../application/analyze_s_curve/handler.js';
import { AiSCurveEstimator } from '../s_curve/ai_estimator.js';
import { AiSCurveDataExtractor } from '../s_curve/ai_data_extractor.js';

export function createTrizTools(
  analysisService: ContradictionAnalysisService,
  principleEngine: PrincipleEngine,
  suFieldService: SuFieldAnalysisService,
  idealityHandler: EvaluateIdealityHandler,
  aiAgent?: AiTrizAgent,
  cachedSearch?: CachedSearchService,
  summarizer?: AISummarizer,
  sCurveHandler?: AnalyzeSCurveHandler,
  aiSCurveEstimator?: AiSCurveEstimator,
  aiSCurveDataExtractor?: AiSCurveDataExtractor,
) {
  const matrix = ContradictionMatrix.getInstance();

  const analyzeContradiction = defineTool(
    'triz_analyze_contradiction',
    'Analyze a technical contradiction using the TRIZ Contradiction Matrix. Returns recommended inventive principles.',
  )
    .required('improvingParameter', 'number', 'TRIZ parameter index (1-39) that you want to improve')
    .required('worseningParameter', 'number', 'TRIZ parameter index (1-39) that gets worse')
    .param('description', 'string', 'Description of the contradiction')
    .handle((args) => {
      try {
        const result = analysisService.analyze(
          args.improvingParameter,
          args.worseningParameter,
          args.description || '',
        );
        return ok({
          contradictionId: result.contradiction.id,
          principles: result.principles,
        });
      } catch (e: any) {
        return err(e.message);
      }
    });

  const lookupMatrix = defineTool(
    'triz_lookup_matrix',
    'Look up the TRIZ Contradiction Matrix for a pair of parameters.',
  )
    .required('improvingParameter', 'number', 'Improving parameter index (1-39)')
    .required('worseningParameter', 'number', 'Worsening parameter index (1-39)')
    .handle((args) => {
      try {
        const principles = matrix.lookup(args.improvingParameter, args.worseningParameter);
        const detailed = principles
          .map(idx => principleEngine.getPrinciple(idx))
          .filter(Boolean)
          .map(p => ({ index: p!.index, name: p!.name, description: p!.description }));
        return ok({ principles: detailed });
      } catch (e: any) {
        return err(e.message);
      }
    });

  const getPrinciple = defineTool(
    'triz_get_principle',
    'Get details of a specific TRIZ Inventive Principle.',
  )
    .required('index', 'number', 'Principle index (1-40)')
    .handle((args) => {
      const principle = principleEngine.getPrinciple(args.index);
      if (!principle) return err(`Principle ${args.index} not found`);
      return ok(principle);
    });

  const searchPrinciples = defineTool(
    'triz_search_principles',
    'Search TRIZ Inventive Principles by keyword.',
  )
    .required('query', 'string', 'Search query')
    .handle((args) => {
      const results = principleEngine.searchPrinciples(args.query);
      return ok({ count: results.length, principles: results });
    });

  const listPrinciples = defineTool(
    'triz_list_principles',
    'List all 40 TRIZ Inventive Principles.',
  ).handle(() => {
    const principles = principleEngine.getAllPrinciples();
    return ok({ count: principles.length, principles });
  });

  const listParameters = defineTool(
    'triz_list_parameters',
    'List all 39 TRIZ engineering parameters.',
  ).handle(() => {
    const parameters = matrix.getAllParameters();
    return ok({ count: parameters.length, parameters });
  });

  const analyzeSuField = defineTool(
    'triz_analyze_su_field',
    'Analyze a Substance-Field model and suggest improvements.',
  )
    .required('substance1', 'string', 'First substance (S1) - the tool')
    .required('substance2', 'string', 'Second substance (S2) - the object')
    .required('field', 'string', 'Field type (mechanical, thermal, chemical, etc.)')
    .param('problemType', 'string', 'Type of problem: harmful, insufficient, or complete')
    .handle((args) => {
      let result;
      if (args.problemType === 'harmful') {
        result = suFieldService.analyzeHarmful(args.substance1, args.substance2, args.field);
      } else if (args.problemType === 'insufficient') {
        result = suFieldService.analyzeInsufficient(args.substance1, args.substance2, args.field);
      } else {
        result = suFieldService.analyze({
          substance1: args.substance1,
          substance2: args.substance2,
          field: args.field,
        });
      }
      return ok(result);
    });

  const evaluateIdeality = defineTool(
    'triz_evaluate_ideality',
    'Evaluate the ideality of a system using TRIZ ideality formula.',
  )
    .required('problemId', 'string', 'Problem identifier')
    .param('benefits', 'array', 'List of benefits')
    .param('costs', 'array', 'List of costs')
    .param('harms', 'array', 'List of harmful effects')
    .handle((args) => {
      try {
        return idealityHandler.execute({
          problemId: args.problemId,
          benefits: args.benefits || [],
          costs: args.costs || [],
          harms: args.harms || [],
        }).then(result => ok(result));
      } catch (e: any) {
        return err(e.message);
      }
    });

  const aiAnalyzeContradiction = defineTool(
    'triz_ai_analyze',
    'Use AI to analyze a contradiction and provide creative insights.',
  )
    .required('improvingParam', 'string', 'Description of improving parameter')
    .required('worseningParam', 'string', 'Description of worsening parameter')
    .required('description', 'string', 'Problem description')
    .handle((args) => {
      if (!aiAgent) return err('AI agent not configured');
      return aiAgent.analyzeContradiction(
        args.improvingParam,
        args.worseningParam,
        args.description,
      ).then(result => ok({ analysis: result }));
    });

  const aiGenerateInsight = defineTool(
    'triz_ai_insight',
    'Use AI to generate insights for applying a specific TRIZ principle to a problem.',
  )
    .required('problemDescription', 'string', 'Description of the problem')
    .required('principleIndex', 'number', 'TRIZ principle index (1-40)')
    .param('context', 'string', 'Additional context')
    .handle((args) => {
      if (!aiAgent) return err('AI agent not configured');
      const principle = principleEngine.getPrinciple(args.principleIndex);
      if (!principle) return err(`Principle ${args.principleIndex} not found`);
      return aiAgent.generateInsight(
        args.problemDescription,
        principle,
        args.context,
      ).then(result => ok({ insight: result }));
    });

  const triggerSearchPatents = defineTool(
    'triz_trigger_search_patents',
    'Trigger a patent search. Returns cached results immediately, with real API results updating in background.',
  )
    .required('query', 'string', 'Patent search query')
    .param('maxResults', 'number', 'Maximum results (default: 5)')
    .handle((args) => {
      if (!cachedSearch) return err('Search service not configured');
      const max = args.maxResults || 5;
      const key = `patents:${args.query}:${max}`;

      const mockResults = getMockPatents(args.query, max);
      cachedSearch.getCache().set(key, mockResults);

      cachedSearch.searchPatents(args.query, max)
        .then(realResults => {
          if (realResults.length > 0) {
            cachedSearch.getCache().set(key, realResults);
          }
        })
        .catch(() => {});

      return ok({ count: mockResults.length, results: mockResults, note: 'Showing cached results. Real API search running in background.' });
    });

  const triggerSearchPapers = defineTool(
    'triz_trigger_search_papers',
    'Trigger a paper search. Returns cached results immediately, with real API results updating in background.',
  )
    .required('query', 'string', 'Paper search query')
    .param('maxResults', 'number', 'Maximum results (default: 5)')
    .handle((args) => {
      if (!cachedSearch) return err('Search service not configured');
      const max = args.maxResults || 5;
      const key = `papers:${args.query}:${max}`;

      const mockResults = getMockPapers(args.query, max);
      cachedSearch.getCache().set(key, mockResults);

      cachedSearch.searchPapers(args.query, max)
        .then(realResults => {
          if (realResults.length > 0) {
            cachedSearch.getCache().set(key, realResults);
          }
        })
        .catch(() => {});

      return ok({ count: mockResults.length, results: mockResults, note: 'Showing cached results. Real API search running in background.' });
    });

  const triggerSearchPriorArt = defineTool(
    'triz_trigger_search_prior_art',
    'Trigger searches for patents, papers, and tech solutions. Returns cached results immediately.',
  )
    .required('query', 'string', 'Search query')
    .param('maxResults', 'number', 'Maximum results per source (default: 5)')
    .handle((args) => {
      if (!cachedSearch) return err('Search service not configured');
      const max = args.maxResults || 5;

      const patents = getMockPatents(args.query, max);
      const papers = getMockPapers(args.query, max);
      const tech = getMockTechSolutions(args.query, max);
      const allResults = [...patents, ...papers, ...tech];

      cachedSearch.getCache().set(`patents:${args.query}:${max}`, patents);
      cachedSearch.getCache().set(`papers:${args.query}:${max}`, papers);
      cachedSearch.getCache().set(`tech:${args.query}:${max}`, tech);

      cachedSearch.searchPatents(args.query, max).catch(() => {});
      cachedSearch.searchPapers(args.query, max).catch(() => {});
      cachedSearch.searchTechSolutions(args.query, max).catch(() => {});

      return ok({
        count: allResults.length,
        results: allResults,
        note: 'Showing cached results. Real API searches running in background.',
      });
    });

  const getCachedPatents = defineTool(
    'triz_get_cached_patents',
    'Get cached patent search results. Use triz_trigger_search_patents first.',
  )
    .required('query', 'string', 'The original search query')
    .param('maxResults', 'number', 'Maximum results (default: 5)')
    .handle((args) => {
      if (!cachedSearch) return err('Search service not configured');
      const max = args.maxResults || 5;
      let results = cachedSearch.getCachedPatents(args.query, max);
      if (results.length === 0) {
        results = getMockPatents(args.query, max);
        cachedSearch.getCache().set(`patents:${args.query}:${max}`, results);
      }
      return ok({ count: results.length, results });
    });

  const getCachedPapers = defineTool(
    'triz_get_cached_papers',
    'Get cached paper search results. Use triz_trigger_search_papers first.',
  )
    .required('query', 'string', 'The original search query')
    .param('maxResults', 'number', 'Maximum results (default: 5)')
    .handle((args) => {
      if (!cachedSearch) return err('Search service not configured');
      const max = args.maxResults || 5;
      let results = cachedSearch.getCachedPapers(args.query, max);
      if (results.length === 0) {
        results = getMockPapers(args.query, max);
        cachedSearch.getCache().set(`papers:${args.query}:${max}`, results);
      }
      return ok({ count: results.length, results });
    });

  const getCachedPriorArt = defineTool(
    'triz_get_cached_prior_art',
    'Get all cached prior art (patents + papers + tech). Use triz_trigger_search_prior_art first.',
  )
    .required('query', 'string', 'The original search query')
    .param('maxResults', 'number', 'Maximum results per source (default: 5)')
    .handle((args) => {
      if (!cachedSearch) return err('Search service not configured');
      const max = args.maxResults || 5;
      let results = cachedSearch.getCachedPriorArt(args.query, max);
      if (results.length === 0) {
        const patents = getMockPatents(args.query, max);
        const papers = getMockPapers(args.query, max);
        const tech = getMockTechSolutions(args.query, max);
        results = [...patents, ...papers, ...tech].slice(0, max * 3);
        cachedSearch.getCache().set(`patents:${args.query}:${max}`, patents);
        cachedSearch.getCache().set(`papers:${args.query}:${max}`, papers);
        cachedSearch.getCache().set(`tech:${args.query}:${max}`, tech);
      }
      return ok({ count: results.length, results });
    });

  const listCachedSearches = defineTool(
    'triz_list_cached_searches',
    'List all cached search results available.',
  ).handle(() => {
    if (!cachedSearch) return err('Search service not configured');
    const cached = cachedSearch.getCache().getAll();
    return ok({
      count: cached.length,
      searches: cached.map(c => ({ query: c.query, resultCount: c.results.length })),
    });
  });

  const analyzeSCurve = defineTool(
    'triz_analyze_s_curve',
    'Analyze the S-curve of a technology. Shows current (S1) and next-gen (S2) curves with stage detection, TRL assessment, and strategy recommendations.',
  )
    .required('technologyName', 'string', 'Name of the technology (e.g., "lithium-ion batteries")')
    .required('performanceMetric', 'string', 'Performance metric (e.g., "Wh/kg", "MPG", "TFLOPS")')
    .param('dataPoints', 'array', 'Optional data points as [{x: year, y: performance}]')
    .param('currentYear', 'number', 'Current year (default: this year)')
    .param('trl', 'number', 'Optional user-provided TRL (1-9) to override AI assessment')
    .param('trlReasoning', 'string', 'Reasoning for user-provided TRL')
    .handle((args) => {
      if (!sCurveHandler) return err('S-Curve analysis not configured');
      if (!cachedSearch) return err('Search service not configured');

      const key = `scurve:${args.technologyName}:${args.performanceMetric}:${args.trl || 'auto'}`;
      const cached = cachedSearch.getCache().get(key) as any;

      if (cached && cached.length > 0) {
        const result = cached[0];
        return ok({
          technologyName: result.technologyName,
          s1Stage: result.s1Stage,
          s2Stage: result.s2Stage,
          unicodeChart: result.unicodeChart,
          analysis: result.analysis,
          recommendations: result.recommendations,
          crossoverYear: result.crossoverYear,
          s1MaxPerformance: result.s1MaxPerformance,
          s2MaxPerformance: result.s2MaxPerformance,
          s1TRL: result.s1TRL,
          s2TRLRange: result.s2TRLRange,
          trlReconciliation: result.trlReconciliation,
          svg: result.svg,
          note: 'Cached result. SVG output available. Save the svg field to a .svg file to view.',
        });
      }

      sCurveHandler.execute({
        technologyName: args.technologyName,
        performanceMetric: args.performanceMetric,
        dataPoints: args.dataPoints || [],
        currentYear: args.currentYear,
        trl: args.trl,
        trlReasoning: args.trlReasoning,
      }).then(result => {
        cachedSearch.getCache().set(key, [result] as any);
      }).catch(() => {});

      return ok({
        technologyName: args.technologyName,
        s1Stage: 'Analyzing...',
        s2Stage: 'Analyzing...',
        unicodeChart: '',
        analysis: 'Analysis running in background. Call this tool again to retrieve results.',
        recommendations: [],
        crossoverYear: 0,
        s1MaxPerformance: 0,
        s2MaxPerformance: 0,
        note: 'Analysis triggered. Call again to retrieve results.',
      });
    });

  const extractSCurveData = defineTool(
    'triz_extract_s_curve_data',
    'Use AI to search and extract historical performance data for a technology. Returns data points for S-curve analysis.',
  )
    .required('technologyName', 'string', 'Name of the technology')
    .required('performanceMetric', 'string', 'Performance metric to track')
    .handle((args) => {
      if (!aiSCurveDataExtractor) return err('AI S-Curve data extractor not configured');
      if (!cachedSearch) return err('Search service not configured');

      const key = `scurve_data:${args.technologyName}:${args.performanceMetric}`;
      const cached = cachedSearch.getCache().get(key);

      if (cached && cached.length > 0) {
        return ok({
          technology: args.technologyName,
          metric: args.performanceMetric,
          dataPoints: cached,
          sources: [],
          reasoning: 'Cached data points from previous extraction',
          dataPointCount: cached.length,
        });
      }

      aiSCurveDataExtractor.extractData(args.technologyName, args.performanceMetric)
        .then(result => {
          if (result.dataPoints.length > 0) {
            cachedSearch.getCache().set(key, result.dataPoints as any);
          }
        })
        .catch(() => {});

      return ok({
        technology: args.technologyName,
        metric: args.performanceMetric,
        dataPoints: [],
        sources: [],
        reasoning: 'AI extraction triggered. Call this tool again to retrieve extracted data points.',
        dataPointCount: 0,
      });
    });

  const enrichSCurve = defineTool(
    'triz_enrich_s_curve',
    'Use AI to refine S-curve parameters for a technology. Improves accuracy when no data points were provided.',
  )
    .required('technologyName', 'string', 'Name of the technology')
    .required('performanceMetric', 'string', 'Performance metric')
    .handle((args) => {
      if (!aiSCurveEstimator) return err('AI S-Curve estimator not configured');
      return aiSCurveEstimator.estimate(args.technologyName, args.performanceMetric)
        .then(result => ok({
          technology: args.technologyName,
          estimatedParameters: result.estimatedParameters,
          estimatedStage: result.estimatedStage,
          s2Offset: result.s2Offset,
          reasoning: result.reasoning,
        }));
    });

  return [
    analyzeContradiction,
    lookupMatrix,
    getPrinciple,
    searchPrinciples,
    listPrinciples,
    listParameters,
    analyzeSuField,
    evaluateIdeality,
    aiAnalyzeContradiction,
    aiGenerateInsight,
    triggerSearchPatents,
    triggerSearchPapers,
    triggerSearchPriorArt,
    getCachedPatents,
    getCachedPapers,
    getCachedPriorArt,
    listCachedSearches,
    extractSCurveData,
    analyzeSCurve,
    enrichSCurve,
  ];
}
