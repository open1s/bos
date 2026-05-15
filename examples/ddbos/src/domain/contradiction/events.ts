export interface DomainEvent {
  type: string;
  aggregateId: string;
  timestamp: Date;
  payload: Record<string, any>;
}

export class ContradictionCreated implements DomainEvent {
  readonly type = 'ContradictionCreated';
  readonly timestamp: Date;

  constructor(
    public readonly aggregateId: string,
    public readonly improvingParameter: number,
    public readonly worseningParameter: number,
    public readonly recommendedPrinciples: number[],
  ) {
    this.timestamp = new Date();
  }

  get payload(): Record<string, any> {
    return {
      improvingParameter: this.improvingParameter,
      worseningParameter: this.worseningParameter,
      recommendedPrinciples: this.recommendedPrinciples,
    };
  }
}

export class ContradictionResolved implements DomainEvent {
  readonly type = 'ContradictionResolved';
  readonly timestamp: Date;

  constructor(
    public readonly aggregateId: string,
    public readonly resolutionNotes: string,
  ) {
    this.timestamp = new Date();
  }

  get payload(): Record<string, any> {
    return { resolutionNotes: this.resolutionNotes };
  }
}

export class SolutionGenerated implements DomainEvent {
  readonly type = 'SolutionGenerated';
  readonly timestamp: Date;

  constructor(
    public readonly aggregateId: string,
    public readonly contradictionId: string,
    public readonly principleIndex: number,
    public readonly solutionDescription: string,
  ) {
    this.timestamp = new Date();
  }

  get payload(): Record<string, any> {
    return {
      contradictionId: this.contradictionId,
      principleIndex: this.principleIndex,
      solutionDescription: this.solutionDescription,
    };
  }
}
