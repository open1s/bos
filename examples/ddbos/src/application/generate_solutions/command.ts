import { ExternalReference } from '../../domain/solution/external_reference.js';

export interface GenerateSolutionsCommand {
  contradictionId: string;
  aiEnhanced?: boolean;
  researchEnabled?: boolean;
  problemDescription?: string;
  maxSolutions?: number;
  maxReferencesPerSolution?: number;
}

export interface GeneratedSolution {
  id: string;
  description: string;
  principleIndex: number;
  principleName: string;
  aiEnhanced: boolean;
  aiInsight?: string;
  references: ExternalReference[];
}

export interface GenerateSolutionsResult {
  contradictionId: string;
  solutions: GeneratedSolution[];
}
