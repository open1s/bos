import { TrizSolution } from './entity.js';

export interface SolutionRepository {
  save(solution: TrizSolution): Promise<void>;
  findById(id: string): Promise<TrizSolution | null>;
  findByContradictionId(contradictionId: string): Promise<TrizSolution[]>;
}
