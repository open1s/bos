import { ContradictionType } from '../../domain/contradiction/value_objects.js';

export interface AnalyzeContradictionCommand {
  improvingParameter: number;
  worseningParameter: number;
  description: string;
  type?: ContradictionType;
  context?: string;
}

export interface AnalyzeContradictionResult {
  contradictionId: string;
  improvingParameter: number;
  worseningParameter: number;
  recommendedPrinciples: Array<{
    index: number;
    name: string;
    description: string;
  }>;
}
