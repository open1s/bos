import { SCurveStage, CurvePoint, CurveParameters, StageBoundary, TRLAssessment, TRLRange, Milestone } from './value_objects.js';

let _idCounter = 0;

function generateId(): string {
  return `sc_${++_idCounter}_${Date.now()}`;
}

export interface SCurveProps {
  technologyName: string;
  performanceMetric: string;
  s1Parameters: CurveParameters;
  s2Parameters: CurveParameters;
  s1Stage: SCurveStage;
  s2Stage: SCurveStage;
  s1Estimated: boolean;
  s2Estimated: boolean;
  dataPoints?: CurvePoint[];
  milestones?: Milestone[];
  s1TRL?: TRLAssessment;
  s2TRLRange?: TRLRange;
  trlReconciliation?: string;
}

export class SCurve {
  public readonly id: string;
  public readonly createdAt: Date;
  private _s1Parameters: CurveParameters;
  private _s2Parameters: CurveParameters;
  private _s1Stage: SCurveStage;
  private _s2Stage: SCurveStage;
  private _s1Estimated: boolean;
  private _s2Estimated: boolean;
  private _dataPoints: CurvePoint[];
  private _s1TRL?: TRLAssessment;
  private _s2TRLRange?: TRLRange;
  private _trlReconciliation?: string;
  private _milestones: Milestone[];

  constructor(
    public readonly technologyName: string,
    public readonly performanceMetric: string,
    s1Parameters: CurveParameters,
    s2Parameters: CurveParameters,
    s1Stage: SCurveStage,
    s2Stage: SCurveStage,
    s1Estimated: boolean,
    s2Estimated: boolean,
    dataPoints?: CurvePoint[],
    milestones?: Milestone[],
    s1TRL?: TRLAssessment,
    s2TRLRange?: TRLRange,
    trlReconciliation?: string,
    id?: string,
    createdAt?: Date,
  ) {
    this.id = id || generateId();
    this.createdAt = createdAt || new Date();
    this._s1Parameters = s1Parameters;
    this._s2Parameters = s2Parameters;
    this._s1Stage = s1Stage;
    this._s2Stage = s2Stage;
    this._s1Estimated = s1Estimated;
    this._s2Estimated = s2Estimated;
    this._dataPoints = dataPoints || [];
    this._milestones = milestones || [];
    this._s1TRL = s1TRL;
    this._s2TRLRange = s2TRLRange;
    this._trlReconciliation = trlReconciliation;
  }

  get s1Parameters(): CurveParameters { return this._s1Parameters; }
  get s2Parameters(): CurveParameters { return this._s2Parameters; }
  get s1Stage(): SCurveStage { return this._s1Stage; }
  get s2Stage(): SCurveStage { return this._s2Stage; }
  get s1Estimated(): boolean { return this._s1Estimated; }
  get s2Estimated(): boolean { return this._s2Estimated; }
  get dataPoints(): ReadonlyArray<CurvePoint> { return [...this._dataPoints]; }
  get milestones(): ReadonlyArray<Milestone> { return [...this._milestones]; }
  get s1TRL(): TRLAssessment | undefined { return this._s1TRL; }
  get s2TRLRange(): TRLRange | undefined { return this._s2TRLRange; }
  get trlReconciliation(): string | undefined { return this._trlReconciliation; }

  generateDataPoints(startX: number, endX: number, steps = 50): CurvePoint[] {
    const points: CurvePoint[] = [];
    const step = (endX - startX) / steps;
    for (let i = 0; i <= steps; i++) {
      const x = startX + i * step;
      const y = this.logistic(x, this._s1Parameters);
      points.push({ x, y });
    }
    return points;
  }

  generateS2DataPoints(startX: number, endX: number, steps = 50): CurvePoint[] {
    const points: CurvePoint[] = [];
    const step = (endX - startX) / steps;
    for (let i = 0; i <= steps; i++) {
      const x = startX + i * step;
      const y = this.logistic(x, this._s2Parameters);
      points.push({ x, y });
    }
    return points;
  }

  private logistic(x: number, params: CurveParameters): number {
    return params.L / (1 + Math.exp(-params.k * (x - params.t0)));
  }

  getS1PerformanceAt(x: number): number {
    return this.logistic(x, this._s1Parameters);
  }

  getS2PerformanceAt(x: number): number {
    return this.logistic(x, this._s2Parameters);
  }

  getCrossoverPoint(): number {
    let x = this._s1Parameters.t0 - 10;
    const end = this._s2Parameters.t0 + 10;
    const step = 0.1;
    while (x < end) {
      const s1y = this.logistic(x, this._s1Parameters);
      const s2y = this.logistic(x, this._s2Parameters);
      if (s2y >= s1y) return x;
      x += step;
    }
    return end;
  }

  getYearsToS1Peak(): number {
    return Math.max(0, this._s1Parameters.t0 - (this._dataPoints.length > 0 ? this._dataPoints[this._dataPoints.length - 1].x : this._s1Parameters.t0));
  }

  toJSON(): SCurveProps {
    return {
      technologyName: this.technologyName,
      performanceMetric: this.performanceMetric,
      s1Parameters: this._s1Parameters,
      s2Parameters: this._s2Parameters,
      s1Stage: this._s1Stage,
      s2Stage: this._s2Stage,
      s1Estimated: this._s1Estimated,
      s2Estimated: this._s2Estimated,
      dataPoints: [...this._dataPoints],
      milestones: [...this._milestones],
      s1TRL: this._s1TRL,
      s2TRLRange: this._s2TRLRange,
      trlReconciliation: this._trlReconciliation,
    };
  }
}
