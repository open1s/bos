import { SolutionType, SolutionEvaluation } from './value_objects.js';

let _idCounter = 0;

function generateId(): string {
  return `sol_${++_idCounter}_${Date.now()}`;
}

export interface TrizSolutionProps {
  description: string;
  type: SolutionType;
  contradictionId?: string;
  problemId?: string;
  principleIndices?: number[];
  evaluation?: SolutionEvaluation;
  notes?: string;
}

export class TrizSolution {
  public readonly id: string;
  public readonly createdAt: Date;
  private _description: string;
  private _type: SolutionType;
  private _contradictionId?: string;
  private _problemId?: string;
  private _principleIndices: number[];
  private _evaluation?: SolutionEvaluation;
  private _notes?: string;
  private _selected: boolean;

  constructor(
    description: string,
    type: SolutionType,
    contradictionId?: string,
    problemId?: string,
    principleIndices?: number[],
    evaluation?: SolutionEvaluation,
    notes?: string,
    id?: string,
    createdAt?: Date,
  ) {
    this.id = id || generateId();
    this.createdAt = createdAt || new Date();
    this._description = description;
    this._type = type;
    this._contradictionId = contradictionId;
    this._problemId = problemId;
    this._principleIndices = principleIndices || [];
    this._evaluation = evaluation;
    this._notes = notes;
    this._selected = false;
  }

  get description(): string { return this._description; }
  get type(): SolutionType { return this._type; }
  get contradictionId(): string | undefined { return this._contradictionId; }
  get problemId(): string | undefined { return this._problemId; }
  get principleIndices(): ReadonlyArray<number> { return [...this._principleIndices]; }
  get evaluation(): SolutionEvaluation | undefined { return this._evaluation; }
  get notes(): string | undefined { return this._notes; }
  get isSelected(): boolean { return this._selected; }

  select(): void {
    this._selected = true;
  }

  updateEvaluation(evaluation: SolutionEvaluation): void {
    this._evaluation = evaluation;
  }

  updateNotes(notes: string): void {
    this._notes = notes;
  }

  get totalScore(): number | undefined {
    if (!this._evaluation) return undefined;
    return (
      this._evaluation.feasibility +
      this._evaluation.novelty +
      this._evaluation.impact -
      this._evaluation.cost
    );
  }

  toJSON(): TrizSolutionProps {
    return {
      description: this._description,
      type: this._type,
      contradictionId: this._contradictionId,
      problemId: this._problemId,
      principleIndices: [...this._principleIndices],
      evaluation: this._evaluation,
      notes: this._notes,
    };
  }
}
