import { TRLAssessment, TRLRange, TRLLevel, Milestone } from '../../domain/s_curve/value_objects.js';

export interface AnalyzeSCurveCommand {
  technologyName: string;
  performanceMetric: string;
  dataPoints?: Array<{ x: number; y: number }>;
  milestones?: Milestone[];
  currentYear?: number;
  trl?: TRLLevel;
  trlReasoning?: string;
}

export interface SCurveResult {
  technologyName: string;
  performanceMetric: string;
  s1Stage: string;
  s2Stage: string;
  s1Estimated: boolean;
  s2Estimated: boolean;
  svg: string;
  unicodeChart: string;
  analysis: string;
  recommendations: string[];
  crossoverYear: number;
  s1MaxPerformance: number;
  s2MaxPerformance: number;
  milestones: Milestone[];
  s1TRL?: TRLAssessment;
  s2TRLRange?: TRLRange;
  trlReconciliation?: string;
}
