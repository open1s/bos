export type SolutionType = 'principle-based' | 'su-field' | 'trend-based' | 'ai-generated';

export interface SolutionEvaluation {
  feasibility: number;
  novelty: number;
  impact: number;
  cost: number;
}
