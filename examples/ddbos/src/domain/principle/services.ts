import { INVENTIVE_PRINCIPLES, InventivePrinciple, getPrincipleByIndex } from '../principle/entity.js';

export interface PrincipleCombination {
  principles: InventivePrinciple[];
  combinedDescription: string;
  synergy: string;
}

export class PrincipleEngine {
  getPrinciple(index: number): InventivePrinciple | undefined {
    return getPrincipleByIndex(index);
  }

  getAllPrinciples(): ReadonlyArray<InventivePrinciple> {
    return INVENTIVE_PRINCIPLES;
  }

  searchPrinciples(query: string): InventivePrinciple[] {
    const lowerQuery = query.toLowerCase();
    return INVENTIVE_PRINCIPLES.filter(
      p =>
        p.name.toLowerCase().includes(lowerQuery) ||
        p.description.toLowerCase().includes(lowerQuery) ||
        p.examples.some(e => e.toLowerCase().includes(lowerQuery)),
    );
  }

  combinePrinciples(indices: number[]): PrincipleCombination {
    const principles = indices
      .map(i => getPrincipleByIndex(i))
      .filter((p): p is InventivePrinciple => p !== undefined);

    if (principles.length === 0) {
      throw new Error('No valid principles found for combination');
    }

    const names = principles.map(p => p.name).join(' + ');
    return {
      principles,
      combinedDescription: `Combined approach using: ${names}`,
      synergy: `Applying ${principles.length} principles together may yield emergent solutions not possible with individual principles alone.`,
    };
  }

  getPrincipleExamples(index: number): string[] {
    const principle = getPrincipleByIndex(index);
    return principle ? principle.examples : [];
  }
}
