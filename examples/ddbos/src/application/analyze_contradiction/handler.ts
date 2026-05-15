import { AnalyzeContradictionCommand, AnalyzeContradictionResult } from './command.js';
import { ContradictionAnalysisService } from '../../domain/contradiction/services.js';
import { ContradictionRepository } from '../../domain/contradiction/repository.js';

export class AnalyzeContradictionHandler {
  constructor(
    private readonly analysisService: ContradictionAnalysisService,
    private readonly repository: ContradictionRepository,
  ) {}

  async execute(command: AnalyzeContradictionCommand): Promise<AnalyzeContradictionResult> {
    const result = this.analysisService.analyze(
      command.improvingParameter,
      command.worseningParameter,
      command.description,
      command.type || 'technical',
      command.context,
    );

    await this.repository.save(result.contradiction);

    return {
      contradictionId: result.contradiction.id,
      improvingParameter: result.contradiction.improvingParameter,
      worseningParameter: result.contradiction.worseningParameter,
      recommendedPrinciples: result.principles,
    };
  }
}
