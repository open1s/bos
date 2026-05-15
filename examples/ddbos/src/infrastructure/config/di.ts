import { BrainOS } from '@open1s/ezbos';
import { ContradictionAnalysisService } from '../../domain/contradiction/services.js';
import { ContradictionMatrix } from '../../domain/contradiction/matrix.js';
import { PrincipleEngine } from '../../domain/principle/services.js';
import { SuFieldAnalysisService } from '../../domain/solution/su_field_service.js';
import { AnalyzeContradictionHandler } from '../../application/analyze_contradiction/handler.js';
import { GenerateSolutionsHandler } from '../../application/generate_solutions/handler.js';
import { EvaluateIdealityHandler } from '../../application/evaluate_ideality/handler.js';
import { InMemoryContradictionRepository } from '../persistence/in_memory_repository.js';
import { InMemorySolutionRepository } from '../persistence/solution_repository.js';
import { AiTrizAgent } from '../ai/triz_ai_agent.js';
import { MultiSourceSearchService, MultiSourceSearchConfig } from '../search/multi_source_search.js';
import { CachedSearchService } from '../search/cached_search.js';
import { AISummarizer } from '../search/ai_summarizer.js';
import { AnalyzeSCurveHandler } from '../../application/analyze_s_curve/handler.js';
import { AiSCurveEstimator } from '../s_curve/ai_estimator.js';
import { AiSCurveDataExtractor } from '../s_curve/ai_data_extractor.js';
import { TRLAssessor } from '../triz/trl_assessor.js';
import { UnifiedResearchService } from '../../application/unified_research/service.js';
import { AIResearchOrchestrator } from '../../application/unified_research/ai_orchestrator.js';
import { createTrizTools } from '../http/triz_tools.js';
import { LocaleConfig, DEFAULT_LOCALE } from '../../domain/shared/i18n.js';

export interface TrizDeps {
  brain: BrainOS;
  analysisService: ContradictionAnalysisService;
  principleEngine: PrincipleEngine;
  suFieldService: SuFieldAnalysisService;
  contradictionRepo: InMemoryContradictionRepository;
  solutionRepo: InMemorySolutionRepository;
  analyzeContradictionHandler: AnalyzeContradictionHandler;
  generateSolutionsHandler: GenerateSolutionsHandler;
  idealityHandler: EvaluateIdealityHandler;
  sCurveHandler: AnalyzeSCurveHandler;
  aiAgent: AiTrizAgent;
  aiSCurveEstimator: AiSCurveEstimator;
  aiSCurveDataExtractor: AiSCurveDataExtractor;
  trlAssessor: TRLAssessor;
  searchService: CachedSearchService;
  summarizer: AISummarizer;
  unifiedResearch: UnifiedResearchService;
  aiResearchOrchestrator: AIResearchOrchestrator;
  tools: any[];
}

export async function composeRoot(options: {
  searchConfig?: MultiSourceSearchConfig;
  locale?: LocaleConfig;
} = {}): Promise<TrizDeps> {
  const locale = options.locale || DEFAULT_LOCALE;
  const brain = new BrainOS();
  await brain.start();

  const matrix = ContradictionMatrix.getInstance();
  const analysisService = new ContradictionAnalysisService();
  const principleEngine = new PrincipleEngine();
  const suFieldService = new SuFieldAnalysisService();

  const contradictionRepo = new InMemoryContradictionRepository();
  const solutionRepo = new InMemorySolutionRepository();

  const analyzeContradictionHandler = new AnalyzeContradictionHandler(
    analysisService,
    contradictionRepo,
  );

  const aiAgent = new AiTrizAgent(brain, 'triz-expert', locale);

  const searchConfig = options.searchConfig || {
    semanticScholar: {},
    crossRef: {
      email: 'triz-tool@example.com',
    },
    openAlex: {},
  };
  const innerSearch = new MultiSourceSearchService(searchConfig);
  const searchService = new CachedSearchService(innerSearch);

  const summarizer = new AISummarizer(brain, locale);

  const generateSolutionsHandler = new GenerateSolutionsHandler(
    contradictionRepo,
    principleEngine,
    solutionRepo,
    aiAgent,
    searchService,
    summarizer,
  );

  const idealityHandler = new EvaluateIdealityHandler(locale);

  const aiSCurveDataExtractor = new AiSCurveDataExtractor(searchService, brain, locale);
  const trlAssessor = new TRLAssessor(brain, locale);
  const sCurveHandler = new AnalyzeSCurveHandler(trlAssessor, locale);

  const aiSCurveEstimator = new AiSCurveEstimator(brain, locale);

  const unifiedResearch = new UnifiedResearchService({
    searchService,
    contradictionService: analysisService,
    principleEngine,
    sCurveHandler,
    trlAssessor,
    dataExtractor: aiSCurveDataExtractor,
    locale,
  });

  const aiResearchOrchestrator = new AIResearchOrchestrator(
    brain,
    searchService,
    summarizer,
    unifiedResearch,
    locale,
  );

  const tools = createTrizTools(
    analysisService,
    principleEngine,
    suFieldService,
    idealityHandler,
    aiAgent,
    searchService,
    summarizer,
    sCurveHandler,
    aiSCurveEstimator,
    aiSCurveDataExtractor,
  );

  return {
    brain,
    analysisService,
    principleEngine,
    suFieldService,
    contradictionRepo,
    solutionRepo,
    analyzeContradictionHandler,
    generateSolutionsHandler,
    idealityHandler,
    sCurveHandler,
    aiAgent,
    aiSCurveEstimator,
    aiSCurveDataExtractor,
    trlAssessor,
    searchService,
    summarizer,
    unifiedResearch,
    aiResearchOrchestrator,
    tools,
  };
}
