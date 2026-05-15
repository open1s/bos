import { ProblemType, IdealityLevel, SuFieldComponents } from './value_objects.js';

let _idCounter = 0;

function generateId(): string {
  return `prob_${++_idCounter}_${Date.now()}`;
}

export interface TrizProblemProps {
  title: string;
  description: string;
  type: ProblemType;
  systemName: string;
  idealityLevel: IdealityLevel;
  suField?: SuFieldComponents;
  constraints?: string[];
}

export class TrizProblem {
  public readonly id: string;
  public readonly createdAt: Date;
  private _title: string;
  private _description: string;
  private _systemName: string;
  private _idealityLevel: IdealityLevel;
  private _suField?: SuFieldComponents;
  private _constraints: string[];
  private _solved: boolean;

  constructor(
    title: string,
    description: string,
    public readonly type: ProblemType,
    systemName: string,
    idealityLevel: IdealityLevel,
    suField?: SuFieldComponents,
    constraints?: string[],
    id?: string,
    createdAt?: Date,
  ) {
    this.id = id || generateId();
    this.createdAt = createdAt || new Date();
    this._title = title;
    this._description = description;
    this._systemName = systemName;
    this._idealityLevel = idealityLevel;
    this._suField = suField;
    this._constraints = constraints || [];
    this._solved = false;
  }

  get title(): string { return this._title; }
  get description(): string { return this._description; }
  get systemName(): string { return this._systemName; }
  get idealityLevel(): IdealityLevel { return this._idealityLevel; }
  get suField(): SuFieldComponents | undefined { return this._suField; }
  get constraints(): ReadonlyArray<string> { return [...this._constraints]; }
  get isSolved(): boolean { return this._solved; }

  markSolved(): void {
    this._solved = true;
  }

  updateIdealityLevel(level: IdealityLevel): void {
    this._idealityLevel = level;
  }

  toJSON(): TrizProblemProps {
    return {
      title: this._title,
      description: this._description,
      type: this.type,
      systemName: this._systemName,
      idealityLevel: this._idealityLevel,
      suField: this._suField,
      constraints: [...this._constraints],
    };
  }
}
