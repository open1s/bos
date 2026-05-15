import { SolutionRepository } from '../../domain/solution/repository.js';
import { TrizSolution } from '../../domain/solution/entity.js';

export class InMemorySolutionRepository implements SolutionRepository {
  private store: Map<string, TrizSolution> = new Map();

  async save(solution: TrizSolution): Promise<void> {
    this.store.set(solution.id, solution);
  }

  async findById(id: string): Promise<TrizSolution | null> {
    return this.store.get(id) || null;
  }

  async findByContradictionId(contradictionId: string): Promise<TrizSolution[]> {
    return Array.from(this.store.values()).filter(
      s => s.contradictionId === contradictionId,
    );
  }
}
