import { ContradictionRepository } from '../../domain/contradiction/repository.js';
import { Contradiction } from '../../domain/contradiction/entity.js';

export class InMemoryContradictionRepository implements ContradictionRepository {
  private store: Map<string, Contradiction> = new Map();

  async save(contradiction: Contradiction): Promise<void> {
    this.store.set(contradiction.id, contradiction);
  }

  async findById(id: string): Promise<Contradiction | null> {
    return this.store.get(id) || null;
  }

  async findAll(): Promise<Contradiction[]> {
    return Array.from(this.store.values());
  }

  async findByParameters(improving: number, worsening: number): Promise<Contradiction[]> {
    return Array.from(this.store.values()).filter(
      c => c.improvingParameter === improving && c.worseningParameter === worsening,
    );
  }

  async delete(id: string): Promise<void> {
    this.store.delete(id);
  }
}
