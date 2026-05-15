import { GenerateSolutionsCommand, GenerateSolutionsResult, GeneratedSolution } from './command.js';
import { ContradictionRepository } from '../../domain/contradiction/repository.js';
import { SolutionRepository } from '../../domain/solution/repository.js';
import { PrincipleEngine } from '../../domain/principle/services.js';
import { TrizSolution } from '../../domain/solution/entity.js';
import { AiTrizAgent } from '../../infrastructure/ai/triz_ai_agent.js';
import { SearchService } from '../../domain/solution/search_port.js';
import { ContentExtractor } from '../../infrastructure/search/content_extractor.js';
import { AISummarizer } from '../../infrastructure/search/ai_summarizer.js';
import { createReference, enrichReferenceWithSummary } from '../../domain/solution/external_reference.js';

export class GenerateSolutionsHandler {
  private contentExtractor = new ContentExtractor();

  constructor(
    private readonly contradictionRepo: ContradictionRepository,
    private readonly principleEngine: PrincipleEngine,
    private readonly solutionRepo: SolutionRepository,
    private readonly aiAgent?: AiTrizAgent,
    private readonly searchService?: SearchService,
    private readonly summarizer?: AISummarizer,
  ) {}

  async execute(command: GenerateSolutionsCommand): Promise<GenerateSolutionsResult> {
    const contradiction = await this.contradictionRepo.findById(command.contradictionId);
    if (!contradiction) {
      throw new Error(`Contradiction not found: ${command.contradictionId}`);
    }

    const maxSolutions = command.maxSolutions || 10;
    const solutions: GenerateSolutionsResult['solutions'] = [];

    for (const principleIndex of contradiction.recommendedPrinciples.slice(0, maxSolutions)) {
      const principle = this.principleEngine.getPrinciple(principleIndex);
      if (!principle) continue;

      let aiInsight: string | undefined;
      if (command.aiEnhanced && this.aiAgent) {
        aiInsight = await this.aiAgent.generateInsight(
          contradiction.description,
          principle,
          contradiction.context,
        );
      }

      let references: GeneratedSolution['references'] = [];
      if (command.researchEnabled && this.searchService) {
        references = await this.enrichWithResearch(
          contradiction.description,
          principle,
          command.problemDescription || contradiction.description,
          command.maxReferencesPerSolution || 3,
        );
      }

      const solution = new TrizSolution(
        principle.description,
        command.researchEnabled ? 'ai-generated' : 'principle-based',
        contradiction.id,
        undefined,
        [principleIndex],
        undefined,
        aiInsight,
      );

      await this.solutionRepo.save(solution);

      solutions.push({
        id: solution.id,
        description: solution.description,
        principleIndex,
        principleName: principle.name,
        aiEnhanced: !!aiInsight,
        aiInsight,
        references,
      });
    }

    return {
      contradictionId: command.contradictionId,
      solutions,
    };
  }

  private async enrichWithResearch(
    problemDescription: string,
    principle: { index: number; name: string; description: string },
    fullProblemContext: string,
    maxRefs: number,
  ): Promise<GeneratedSolution['references']> {
    if (!this.searchService || !this.summarizer) return [];

    const searchQuery = `${principle.name} ${problemDescription} TRIZ`;

    const results = await this.searchService.search({
      keywords: [principle.name, problemDescription, 'TRIZ', 'solution'],
      sourceTypes: ['patent', 'paper', 'tech_solution'],
      maxResults: maxRefs * 2,
    });

    const references: GenerateSolutionsResult['solutions'][number]['references'] = [];

    for (const result of results.slice(0, maxRefs)) {
      const relevance = await this.summarizer.assessRelevance(
        `${result.title}: ${result.snippet}`,
        fullProblemContext,
      );

      if (relevance < 30) continue;

      let summary: string | undefined;
      let fetchFull = false;

      if (relevance >= 70) {
        const extracted = await this.contentExtractor.extract(result.url);
        if (extracted) {
          const fullSummary = await this.summarizer.summarizeFullContent(
            extracted.title,
            extracted.mainContent,
            fullProblemContext,
          );
          summary = fullSummary.summary;
          fetchFull = true;
        }
      }

      if (!summary) {
        const snippetSummary = await this.summarizer.summarizeSnippet(
          result.title,
          result.snippet,
          fullProblemContext,
        );
        summary = snippetSummary.summary;
      }

      const ref = createReference(
        result.url,
        result.title,
        result.sourceType,
        result.snippet,
        relevance,
        result.publishedDate,
        result.authors,
      );

      references.push(enrichReferenceWithSummary(ref, summary));
    }

    return references;
  }
}
