import { CurveParameters, SCurveStage, CurvePoint, StageBoundary, STAGE_DESCRIPTIONS, STAGE_STRATEGIES, Milestone } from './value_objects.js';
import { SCurve } from './entity.js';

export interface CurveFittingResult {
  parameters: CurveParameters;
  estimated: boolean;
}

export interface StageDetectionResult {
  stage: SCurveStage;
  confidence: number;
  reasoning: string;
}

export class CurveFittingService {
  fit(dataPoints: CurvePoint[]): CurveFittingResult {
    if (dataPoints.length < 2) {
      return this.estimateFromSparseData(dataPoints);
    }

    const sorted = [...dataPoints].sort((a, b) => a.x - b.x);
    const minX = sorted[0].x;
    const maxX = sorted[sorted.length - 1].x;
    const maxY = Math.max(...sorted.map(p => p.y));
    const minY = Math.min(...sorted.map(p => p.y));

    // Estimate L (carrying capacity) - should be above the highest observed value
    // Use the last few data points to detect if we're approaching a ceiling
    const recentPoints = sorted.slice(-3);
    const recentGrowth = recentPoints.length >= 2
      ? (recentPoints[recentPoints.length - 1].y - recentPoints[0].y) / recentPoints[0].y
      : 1;

    // If recent growth is slowing (< 20%), we're near maturity - L is closer to maxY
    // If still growing fast, L should be higher
    const L = recentGrowth < 0.2 ? maxY * 1.1 : maxY * 1.5;

    // Estimate t0 (inflection point) - where growth is fastest
    // Find the point with maximum growth rate
    let maxGrowthRate = 0;
    let t0 = minX + (maxX - minX) * 0.5; // default to midpoint

    for (let i = 1; i < sorted.length; i++) {
      const dx = sorted[i].x - sorted[i - 1].x;
      const dy = sorted[i].y - sorted[i - 1].y;
      if (dx > 0) {
        const rate = dy / dx;
        if (rate > maxGrowthRate) {
          maxGrowthRate = rate;
          t0 = sorted[i - 1].x + dx * 0.5;
        }
      }
    }

    // Estimate k (growth rate) from the data
    // k determines how steep the S-curve is
    const range = maxX - minX || 1;
    // For a logistic curve, the transition from 10% to 90% of L takes about 4.4/k years
    // Estimate how many years the main growth phase took
    const growthPhaseYears = range * 0.6; // assume 60% of range is growth phase
    const k = 4.4 / growthPhaseYears;

    return {
      parameters: { L, k, t0 },
      estimated: false,
    };
  }

  estimateFromSparseData(dataPoints: CurvePoint[]): CurveFittingResult {
    const now = new Date().getFullYear();

    if (dataPoints.length === 0) {
      // Generate a full lifecycle curve spanning 30 years
      return {
        parameters: {
          L: 300,
          k: 0.15,
          t0: now - 10, // inflection point 10 years ago (maturity phase)
        },
        estimated: true,
      };
    }

    const sorted = [...dataPoints].sort((a, b) => a.x - b.x);
    const currentY = sorted[sorted.length - 1].y;
    const currentX = sorted[sorted.length - 1].x;
    const firstX = sorted[0].x;
    const span = currentX - firstX;

    return {
      parameters: {
        L: currentY * 1.3,
        k: span > 0 ? 4.4 / (span * 0.6) : 0.15,
        t0: firstX + span * 0.4,
      },
      estimated: true,
    };
  }

  estimateS2FromS1(s1Params: CurveParameters, currentYear: number): CurveParameters {
    const s2Offset = 10;
    return {
      L: s1Params.L * 1.5,
      k: s1Params.k * 1.2,
      t0: currentYear + s2Offset,
    };
  }
}

export class StageDetectionService {
  detect(params: CurveParameters, currentYear: number, dataPoints?: CurvePoint[]): StageDetectionResult {
    if (!dataPoints || dataPoints.length < 2) {
      return this.estimateStage(params, currentYear);
    }

    const sorted = [...dataPoints].sort((a, b) => a.x - b.x);
    const currentY = sorted[sorted.length - 1].y;
    const prevY = sorted.length > 1 ? sorted[sorted.length - 2].y : currentY;
    const growthRate = prevY > 0 ? (currentY - prevY) / prevY : 0;

    const performanceRatio = currentY / params.L;

    // Use both performance ratio AND growth rate for more accurate detection
    if (performanceRatio < 0.15) {
      return {
        stage: 'infancy',
        confidence: 0.7,
        reasoning: `Performance at ${Math.round(performanceRatio * 100)}% of theoretical max. Slow growth rate (${(growthRate * 100).toFixed(1)}%). Early R&D phase.`,
      };
    }

    if (performanceRatio <= 0.55 && growthRate > 0.05) {
      return {
        stage: 'growth',
        confidence: 0.85,
        reasoning: `Performance at ${Math.round(performanceRatio * 100)}% of theoretical max. Strong growth rate (${(growthRate * 100).toFixed(1)}%). Rapid improvement phase.`,
      };
    }

    if (performanceRatio < 0.85) {
      return {
        stage: 'maturity',
        confidence: 0.8,
        reasoning: `Performance at ${Math.round(performanceRatio * 100)}% of theoretical max. Growth slowing (${(growthRate * 100).toFixed(1)}%). Diminishing returns setting in.`,
      };
    }

    return {
      stage: 'decline',
      confidence: 0.75,
      reasoning: `Performance near ceiling (${Math.round(performanceRatio * 100)}% of max). Little room for improvement. Technology being replaced.`,
    };
  }

  estimateStage(params: CurveParameters, currentYear: number): StageDetectionResult {
    const performanceRatio = this.logistic(currentYear, params) / params.L;

    if (performanceRatio < 0.15) {
      return {
        stage: 'infancy',
        confidence: 0.5,
        reasoning: `Estimated at ${Math.round(performanceRatio * 100)}% of theoretical max. Early stage based on curve parameters.`,
      };
    }
    if (performanceRatio <= 0.55) {
      return {
        stage: 'growth',
        confidence: 0.6,
        reasoning: `Estimated at ${Math.round(performanceRatio * 100)}% of theoretical max. Growth phase based on curve parameters.`,
      };
    }
    if (performanceRatio < 0.85) {
      return {
        stage: 'maturity',
        confidence: 0.6,
        reasoning: `Estimated at ${Math.round(performanceRatio * 100)}% of theoretical max. Maturity phase based on curve parameters.`,
      };
    }
    return {
      stage: 'decline',
      confidence: 0.5,
      reasoning: `Estimated at ${Math.round(performanceRatio * 100)}% of theoretical max. Decline phase based on curve parameters.`,
    };
  }

  generateStageBoundaries(params: CurveParameters, minX: number, maxX: number): StageBoundary[] {
    const L = params.L;
    const inflectionX = params.t0;

    const inflectionY = this.logistic(inflectionX, params);
    const earlyX = params.t0 - 3 / params.k;
    const lateX = params.t0 + 3 / params.k;

    return [
      {
        stage: 'infancy',
        startX: minX,
        endX: Math.min(inflectionX - (inflectionX - earlyX) * 0.5, maxX),
        performanceRange: [0, this.logistic(Math.min(inflectionX - (inflectionX - earlyX) * 0.5, maxX), params)],
      },
      {
        stage: 'growth',
        startX: Math.min(inflectionX - (inflectionX - earlyX) * 0.5, maxX),
        endX: Math.min(inflectionX + (lateX - inflectionX) * 0.5, maxX),
        performanceRange: [
          this.logistic(Math.min(inflectionX - (inflectionX - earlyX) * 0.5, maxX), params),
          this.logistic(Math.min(inflectionX + (lateX - inflectionX) * 0.5, maxX), params),
        ],
      },
      {
        stage: 'maturity',
        startX: Math.min(inflectionX + (lateX - inflectionX) * 0.5, maxX),
        endX: Math.min(lateX + 5, maxX),
        performanceRange: [
          this.logistic(Math.min(inflectionX + (lateX - inflectionX) * 0.5, maxX), params),
          this.logistic(Math.min(lateX + 5, maxX), params),
        ],
      },
      {
        stage: 'decline',
        startX: Math.min(lateX + 5, maxX),
        endX: maxX,
        performanceRange: [
          this.logistic(Math.min(lateX + 5, maxX), params),
          L,
        ],
      },
    ];
  }

  private logistic(x: number, params: CurveParameters): number {
    return params.L / (1 + Math.exp(-params.k * (x - params.t0)));
  }
}

export class SCurveAnalysisService {
  private fittingService = new CurveFittingService();
  private stageService = new StageDetectionService();

  analyze(
    technologyName: string,
    performanceMetric: string,
    dataPoints: CurvePoint[],
    currentYear: number,
    milestones?: Milestone[],
  ): SCurve {
    const s1Fit = this.fittingService.fit(dataPoints);
    const s1Stage = this.stageService.detect(s1Fit.parameters, currentYear, dataPoints.length >= 2 ? dataPoints : undefined);

    const s2Params = this.fittingService.estimateS2FromS1(s1Fit.parameters, currentYear);
    const s2Stage = this.stageService.estimateStage(s2Params, currentYear);

    return new SCurve(
      technologyName,
      performanceMetric,
      s1Fit.parameters,
      s2Params,
      s1Stage.stage,
      s2Stage.stage,
      s1Fit.estimated,
      true,
      dataPoints,
      milestones,
    );
  }

  generateRecommendations(sCurve: SCurve, currentYear: number): string[] {
    const recommendations: string[] = [];
    const s1Stage = sCurve.s1Stage;
    const s2Stage = sCurve.s2Stage;

    recommendations.push(`**${sCurve.technologyName}** is in the **${s1Stage}** stage of its S-curve.`);
    recommendations.push(STAGE_STRATEGIES[s1Stage]);

    if (s1Stage === 'maturity' || s1Stage === 'decline') {
      recommendations.push(`**Critical:** The S2 curve (${sCurve.performanceMetric} next-gen) is in **${s2Stage}** stage. Begin transitioning resources to S2 technology immediately.`);
    } else if (s1Stage === 'growth') {
      recommendations.push(`Monitor S2 curve development. Currently in **${s2Stage}** stage. Start exploratory R&D for next-generation technology.`);
    }

    const crossover = sCurve.getCrossoverPoint();
    if (crossover < currentYear + 15) {
      recommendations.push(`**S2 crossover predicted in ~${Math.round(crossover - currentYear)} years.** S2 performance will exceed S1 around year ${Math.round(crossover)}.`);
    }

    return recommendations;
  }
}
