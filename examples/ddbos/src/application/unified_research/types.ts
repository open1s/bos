import { SearchResult } from '../../domain/solution/search_port.js';
import { Milestone } from '../../domain/s_curve/value_objects.js';

export interface UnifiedResearchRequest {
  problemDescription: string;
  improvingParameter?: string | number;
  worseningParameter?: string | number;
  technologyName?: string;
  performanceMetric?: string;
  searchQuery?: string;
  maxSearchResults?: number;
  onProgress?: (step: string, message: string) => void;
}

export interface ResearchError {
  component: string;
  message: string;
  severity: 'warning' | 'error';
  timestamp: number;
}

export interface ResearchMetadata {
  startedAt: number;
  completedAt: number;
  durationMs: number;
  sourcesUsed: string[];
  cacheHits: number;
  cacheMisses: number;
  aiCallsMade: number;
}

export interface PriorArtItem extends SearchResult {
  relevanceScore?: number;
  summary?: SummarizationResult;
  sourceType: 'patent' | 'paper' | 'tech_solution';
}

export interface SummarizationResult {
  summary: string;
  keyFindings: string[];
  relevanceToProblem: string;
  trizPrinciples: string[];
  confidence?: number;
}

export interface UnifiedResearchResult {
  summary: string;
  contradictionAnalysis?: {
    improvingParameter: string | number;
    worseningParameter: string | number;
    principles: Array<{ index: number; name: string; description: string }>;
    contradictionId: string;
  };
  priorArt: {
    patents: PriorArtItem[];
    papers: PriorArtItem[];
    techSolutions: PriorArtItem[];
  };
  technologyMaturity?: {
    sCurveStage: string;
    sCurveStageNext: string;
    crossoverYear: number;
    trl: {
      level: number;
      title: string;
      confidence: number;
      isEstimated: boolean;
    };
    trlNext: {
      min: number;
      max: number;
      mostLikely: number;
    };
    sCurveData: {
      isEstimated: boolean;
      dataPointCount: number;
      confidence?: number;
    };
    svgPath?: string;
    unicodeChart?: string;
    milestones?: Milestone[];
  };
  recommendations: string[];
  errors: ResearchError[];
  metadata?: ResearchMetadata;
}
