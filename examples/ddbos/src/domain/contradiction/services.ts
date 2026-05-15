import { ContradictionMatrix } from './matrix.js';
import { Contradiction } from './entity.js';
import { ContradictionType } from './value_objects.js';
import { getPrincipleByIndex } from '../principle/entity.js';

export interface ContradictionAnalysisResult {
  contradiction: Contradiction;
  principles: Array<{
    index: number;
    name: string;
    description: string;
  }>;
}

export class ContradictionAnalysisService {
  private readonly matrix = ContradictionMatrix.getInstance();

  analyze(
    improvingParameter: number,
    worseningParameter: number,
    description: string,
    type: ContradictionType = 'technical',
    context?: string,
  ): ContradictionAnalysisResult {
    const principleIndices = this.matrix.lookup(improvingParameter, worseningParameter);

    const principles = principleIndices
      .map(idx => getPrincipleByIndex(idx))
      .filter((p): p is NonNullable<typeof p> => p !== undefined)
      .map(p => ({
        index: p.index,
        name: p.name,
        description: p.description,
      }));

    const contradiction = new Contradiction(
      improvingParameter,
      worseningParameter,
      description,
      type,
      principleIndices,
      context,
    );

    return { contradiction, principles };
  }

  suggestPrinciples(improvingParameter: number, worseningParameter: number): ReadonlyArray<number> {
    return this.matrix.lookup(improvingParameter, worseningParameter);
  }
}
