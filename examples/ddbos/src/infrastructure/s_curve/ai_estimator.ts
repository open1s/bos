import { Agent, BrainOS } from '@open1s/ezbos';
import { streamAgent } from '../ai/streaming.js';
import { CurvePoint, CurveParameters } from '../../domain/s_curve/value_objects.js';
import { LocaleConfig, DEFAULT_LOCALE, getLanguagePrompt } from '../../domain/shared/i18n.js';

export interface AiEstimateResult {
  estimatedParameters: CurveParameters;
  estimatedStage: 'infancy' | 'growth' | 'maturity' | 'decline';
  s2Offset: number;
  reasoning: string;
}

export class AiSCurveEstimator {
  private agent: Agent | null = null;
  private brain: BrainOS | null = null;
  private locale: LocaleConfig;

  constructor(brain?: BrainOS, locale?: LocaleConfig) {
    this.brain = brain || null;
    this.locale = locale || DEFAULT_LOCALE;
  }

  async initialize(): Promise<void> {
    if (!this.brain) {
      this.brain = new BrainOS();
      await this.brain.start();
    }

    const builder = this.brain.agent('triz-scurve-estimator')
      .with_systemPrompt(`You are a TRIZ S-Curve analysis expert. You estimate S-curve parameters for technologies based on domain knowledge.

Given a technology name and optional performance metric, estimate:
1. L (carrying capacity / max performance)
2. k (growth rate)
3. t0 (inflection point year)
4. Current S-curve stage (infancy, growth, maturity, decline)
5. S2 offset (years until next-gen technology inflection)

Return ONLY a JSON object with these fields. No explanation.

${getLanguagePrompt(this.locale.language)}`)
      .with_temperature(0.3);

    this.agent = await builder.start();
  }

  async estimate(
    technologyName: string,
    performanceMetric: string,
    dataPoints?: CurvePoint[],
  ): Promise<AiEstimateResult> {
    if (!this.agent) await this.initialize();

    const dataStr = dataPoints && dataPoints.length > 0
      ? `\nKnown data points: ${JSON.stringify(dataPoints)}`
      : '';

    const prompt = `Estimate S-curve parameters for:

Technology: ${technologyName}
Performance Metric: ${performanceMetric}${dataStr}

Return JSON with: L, k, t0, estimatedStage, s2Offset, reasoning

${getLanguagePrompt(this.locale.language)}`;

    const response = await streamAgent(this.agent!, prompt);
    return this.parseResponse(response, technologyName);
  }

  private parseResponse(response: string, technologyName: string): AiEstimateResult {
    try {
      const jsonMatch = response.match(/\{[\s\S]*\}/);
      if (jsonMatch) {
        const parsed = JSON.parse(jsonMatch[0]);
        return {
          estimatedParameters: {
            L: parsed.L || 100,
            k: parsed.k || 0.3,
            t0: parsed.t0 || new Date().getFullYear() + 5,
          },
          estimatedStage: parsed.estimatedStage || 'growth',
          s2Offset: parsed.s2Offset || 10,
          reasoning: parsed.reasoning || `AI estimation for ${technologyName}`,
        };
      }
    } catch {
    }

    return this.getDefaultEstimate(technologyName);
  }

  private getDefaultEstimate(technologyName: string): AiEstimateResult {
    return {
      estimatedParameters: { L: 100, k: 0.3, t0: new Date().getFullYear() + 5 },
      estimatedStage: 'growth',
      s2Offset: 10,
      reasoning: `Default estimate for ${technologyName}. Provide data points for accurate analysis.`,
    };
  }

  async close(): Promise<void> {
    if (this.agent) {
      await this.agent.close();
      this.agent = null;
    }
  }
}
