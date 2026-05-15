import { ContradictionType } from './value_objects.js';

let _idCounter = 0;

function generateId(): string {
  return `ctr_${++_idCounter}_${Date.now()}`;
}

export interface ContradictionProps {
  improvingParameter: number;
  worseningParameter: number;
  description: string;
  type: ContradictionType;
  context?: string;
  recommendedPrinciples: number[];
}

export class Contradiction {
  public readonly id: string;
  public readonly createdAt: Date;
  private _description: string;
  private _context?: string;
  private _resolved: boolean;
  private _resolutionNotes?: string;

  constructor(
    public readonly improvingParameter: number,
    public readonly worseningParameter: number,
    description: string,
    public readonly type: ContradictionType,
    public readonly recommendedPrinciples: ReadonlyArray<number>,
    context?: string,
    id?: string,
    createdAt?: Date,
  ) {
    this.id = id || generateId();
    this.createdAt = createdAt || new Date();
    this._description = description;
    this._context = context;
    this._resolved = false;
  }

  get description(): string {
    return this._description;
  }

  get context(): string | undefined {
    return this._context;
  }

  get isResolved(): boolean {
    return this._resolved;
  }

  get resolutionNotes(): string | undefined {
    return this._resolutionNotes;
  }

  resolve(notes: string): void {
    this._resolved = true;
    this._resolutionNotes = notes;
  }

  updateDescription(description: string): void {
    this._description = description;
  }

  updateContext(context: string): void {
    this._context = context;
  }

  toJSON(): ContradictionProps {
    return {
      improvingParameter: this.improvingParameter,
      worseningParameter: this.worseningParameter,
      description: this._description,
      type: this.type,
      context: this._context,
      recommendedPrinciples: [...this.recommendedPrinciples],
    };
  }
}
