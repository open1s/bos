import { EvaluateIdealityCommand, EvaluateIdealityResult } from './command.js';
import { LocaleConfig, DEFAULT_LOCALE, t } from '../../domain/shared/i18n.js';

export class EvaluateIdealityHandler {
  private locale: LocaleConfig;

  constructor(locale?: LocaleConfig) {
    this.locale = locale || DEFAULT_LOCALE;
  }

  async execute(command: EvaluateIdealityCommand): Promise<EvaluateIdealityResult> {
    const lang = this.locale.language;
    const benefitScore = command.benefits.length * 10;
    const costScore = command.costs.length * 5;
    const harmScore = command.harms.length * 8;

    const denominator = costScore + harmScore;
    const score = denominator === 0 ? 100 : Math.round((benefitScore / denominator) * 100);

    let level: 'low' | 'medium' | 'high' | 'ideal';
    if (score >= 80) level = 'ideal';
    else if (score >= 50) level = 'high';
    else if (score >= 25) level = 'medium';
    else level = 'low';

    const recommendations: string[] = [];
    if (command.costs.length > 0) {
      recommendations.push(t('reducingCosts', lang));
    }
    if (command.harms.length > 0) {
      recommendations.push(t('eliminatingHarms', lang));
    }
    if (command.benefits.length < 3) {
      recommendations.push(t('increasingBenefits', lang));
    }
    if (level === 'low' || level === 'medium') {
      recommendations.push(t('considerPrinciples', lang));
    }

    return {
      problemId: command.problemId,
      ideality: {
        score,
        level,
        breakdown: {
          benefits: benefitScore,
          costs: costScore,
          harms: harmScore,
        },
        recommendations,
      },
    };
  }
}
