import { AnalyzeContradictionCommand, AnalyzeContradictionResult } from '../analyze_contradiction/command.js';
import { GenerateSolutionsCommand, GenerateSolutionsResult } from '../generate_solutions/command.js';
import { EvaluateIdealityCommand, EvaluateIdealityResult } from '../evaluate_ideality/command.js';

export interface TrizApplicationService {
  analyzeContradiction(command: AnalyzeContradictionCommand): Promise<AnalyzeContradictionResult>;
  generateSolutions(command: GenerateSolutionsCommand): Promise<GenerateSolutionsResult>;
  evaluateIdeality(command: EvaluateIdealityCommand): Promise<EvaluateIdealityResult>;
}
