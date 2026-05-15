export interface ContradictionRepository {
  save(contradiction: import('./entity.js').Contradiction): Promise<void>;
  findById(id: string): Promise<import('./entity.js').Contradiction | null>;
  findAll(): Promise<import('./entity.js').Contradiction[]>;
  findByParameters(improving: number, worsening: number): Promise<import('./entity.js').Contradiction[]>;
  delete(id: string): Promise<void>;
}
