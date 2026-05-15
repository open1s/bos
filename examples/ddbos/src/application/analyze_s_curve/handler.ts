import { AnalyzeSCurveCommand, SCurveResult } from './command.js';
import { SCurveAnalysisService } from '../../domain/s_curve/services.js';
import { SvgCurveGenerator } from '../../domain/s_curve/svg_generator.js';
import { STAGE_LABELS, STAGE_DESCRIPTIONS, STAGE_STRATEGIES, formatTRL, formatTRLRange } from '../../domain/s_curve/value_objects.js';
import { TRLAssessor, TRLAssessmentInput } from '../../infrastructure/triz/trl_assessor.js';
import { LocaleConfig, DEFAULT_LOCALE, stageLabel, stageDesc, stageStrategy, trlTitle, t, svgLabel } from '../../domain/shared/i18n.js';

export class AnalyzeSCurveHandler {
  private analysisService = new SCurveAnalysisService();
  private svgGenerator = new SvgCurveGenerator();
  private trlAssessor?: TRLAssessor;
  private locale: LocaleConfig;

  constructor(trlAssessor?: TRLAssessor, locale?: LocaleConfig) {
    this.trlAssessor = trlAssessor;
    this.locale = locale || DEFAULT_LOCALE;
  }

  async execute(command: AnalyzeSCurveCommand): Promise<SCurveResult> {
    const currentYear = command.currentYear || new Date().getFullYear();
    const dataPoints = command.dataPoints || [];
    const milestones = command.milestones || [];

    const sCurve = this.analysisService.analyze(
      command.technologyName,
      command.performanceMetric,
      dataPoints,
      currentYear,
      milestones,
    );

    let s1TRL = sCurve.s1TRL;
    let s2TRLRange = sCurve.s2TRLRange;
    let trlReconciliation = sCurve.trlReconciliation;

    if (this.trlAssessor && (!s1TRL || command.trl)) {
      const trlInput: TRLAssessmentInput = {
        technologyName: command.technologyName,
        performanceMetric: command.performanceMetric,
        searchResults: [],
        userProvidedTRL: command.trl,
        userReasoning: command.trlReasoning,
        s1Stage: sCurve.s1Stage,
        s2Stage: sCurve.s2Stage,
      };

      const trlResult = await this.trlAssessor.assess(trlInput);
      s1TRL = trlResult.s1TRL;
      s2TRLRange = trlResult.s2TRLRange;
      trlReconciliation = trlResult.reconciliation;
    }

    const svg = this.svgGenerator.generate(sCurve, {
      showAnnotations: true,
      showLegend: true,
      showStageLabels: true,
      locale: this.locale,
    });

    const unicodeChart = this.svgGenerator.generateUnicodeChart(sCurve, 60, 20, this.locale);

    const recommendations = this.analysisService.generateRecommendations(sCurve, currentYear);

    const analysis = this.buildAnalysis(sCurve, currentYear, s1TRL, s2TRLRange, trlReconciliation);

    const crossover = sCurve.getCrossoverPoint();

    return {
      technologyName: sCurve.technologyName,
      performanceMetric: sCurve.performanceMetric,
      s1Stage: stageLabel(sCurve.s1Stage, this.locale.language),
      s2Stage: stageLabel(sCurve.s2Stage, this.locale.language),
      s1Estimated: sCurve.s1Estimated,
      s2Estimated: sCurve.s2Estimated,
      svg,
      unicodeChart,
      analysis,
      recommendations,
      crossoverYear: Math.round(crossover),
      s1MaxPerformance: Math.round(sCurve.s1Parameters.L),
      s2MaxPerformance: Math.round(sCurve.s2Parameters.L),
      milestones: sCurve.milestones,
      s1TRL,
      s2TRLRange,
      trlReconciliation,
    };
  }

  private buildAnalysis(
    sCurve: import('../../domain/s_curve/entity.js').SCurve,
    currentYear: number,
    s1TRL?: import('../../domain/s_curve/value_objects.js').TRLAssessment,
    s2TRLRange?: import('../../domain/s_curve/value_objects.js').TRLRange,
    trlReconciliation?: string,
  ): string {
    const lang = this.locale.language;
    const lines: string[] = [];

    lines.push(`## ${svgLabel('scurveAnalysis', lang)}: ${sCurve.technologyName}`);
    lines.push('');
    lines.push(`**${t('metric', lang)}:** ${sCurve.performanceMetric}`);
    lines.push(`**${t('date', lang)}:** ${currentYear}`);
    lines.push('');

    lines.push(`### ${t('s1CurrentTech', lang)}`);
    lines.push(`- **${t('sCurveStage', lang)}:** ${stageLabel(sCurve.s1Stage, lang)}${sCurve.s1Estimated ? ` (${t('aiEstimate', lang)})` : ` (${t('real', lang)})`}`);
    if (s1TRL) {
      lines.push(`- **TRL:** ${s1TRL.level}/9 - ${trlTitle(s1TRL.level, lang)}${s1TRL.isUserProvided ? ` (${t('userProvided', lang) || 'user-provided'})` : ` (AI, ${t('confidence', lang)}: ${Math.round(s1TRL.confidence * 100)}%)`}`);
    }
    lines.push(`- **${t('description', lang) || 'Description'}:** ${stageDesc(sCurve.s1Stage, lang)}`);
    lines.push(`- **${t('maxS1', lang)}:** ${Math.round(sCurve.s1Parameters.L)} ${sCurve.performanceMetric}`);
    lines.push(`- **${t('growthRate', lang) || 'Growth Rate'} (k):** ${sCurve.s1Parameters.k.toFixed(3)}`);
    lines.push(`- **${t('inflectionPoint', lang) || 'Inflection Point'}:** ${t('year', lang)} ${Math.round(sCurve.s1Parameters.t0)}`);
    lines.push(`- **${t('strategy', lang)}:** ${stageStrategy(sCurve.s1Stage, lang)}`);
    lines.push('');

    lines.push(`### ${t('s2NextGenTech', lang)}`);
    lines.push(`- **${t('sCurveStage', lang)}:** ${stageLabel(sCurve.s2Stage, lang)} (${t('aiEstimate', lang)})`);
    if (s2TRLRange) {
      lines.push(`- **TRL ${t('value', lang) || 'Range'}:** ${s2TRLRange.min}-${s2TRLRange.max}/9 (${t('mostLikely', lang) || 'most likely'}: ${s2TRLRange.mostLikely})`);
    }
    lines.push(`- **${t('description', lang) || 'Description'}:** ${stageDesc(sCurve.s2Stage, lang)}`);
    lines.push(`- **${t('maxS2', lang)}:** ${Math.round(sCurve.s2Parameters.L)} ${sCurve.performanceMetric}`);
    lines.push(`- **${t('growthRate', lang) || 'Growth Rate'} (k):** ${sCurve.s2Parameters.k.toFixed(3)}`);
    lines.push(`- **${t('inflectionPoint', lang) || 'Inflection Point'}:** ${t('year', lang)} ${Math.round(sCurve.s2Parameters.t0)}`);
    lines.push(`- **${t('strategy', lang)}:** ${stageStrategy(sCurve.s2Stage, lang)}`);
    lines.push('');

    const crossover = sCurve.getCrossoverPoint();
    const yearsToCrossover = Math.round(crossover - currentYear);
    lines.push(`### ${t('strategicInsight', lang)}`);
    lines.push(`- **${t('sCurveCrossover', lang)}:** ~${t('year', lang)} ${Math.round(crossover)} (${yearsToCrossover > 0 ? `${t('inYears', lang)} ${yearsToCrossover} ${t('years', lang)}` : `${Math.abs(yearsToCrossover)} ${t('yearsAgo', lang)}`})`);
    lines.push(`- **${t('performanceGap', lang)}:** S2 ${t('maxS2', lang)} ${Math.round((sCurve.s2Parameters.L / sCurve.s1Parameters.L - 1) * 100)}% ${t('higherThanS1', lang)}`);
    lines.push('');

    if (trlReconciliation) {
      lines.push(`### ${t('trlReconciliation', lang)}`);
      lines.push(trlReconciliation);
      lines.push('');
    }

    if (sCurve.s1Stage === 'maturity' || sCurve.s1Stage === 'decline') {
      lines.push(`⚠️ **${t('urgent', lang)}:** ${t('yourTech', lang) || 'Your technology'} ${t('isInStage', lang) || 'is in the'} ${stageLabel(sCurve.s1Stage, lang)} ${t('stage', lang)}. ` +
        `${t('surpassByYear', lang)} ${Math.round(crossover)}. ` +
        `${t('beginTransitioning', lang)}`);
    } else if (sCurve.s1Stage === 'growth') {
      lines.push(`📈 **${t('opportunity', lang)}:** ${t('yourTech', lang) || 'Your technology'} ${t('isInStage', lang) || 'is in the'} ${stageLabel(sCurve.s1Stage, lang)} ${t('stage', lang)}. ` +
        `${t('continueInvesting', lang)}`);
    } else {
      lines.push(`🔬 **${t('earlyStage', lang)}:** ${t('yourTech', lang) || 'Your technology'} ${t('isInStage', lang) || 'is in the'} ${stageLabel(sCurve.s1Stage, lang)} ${t('stage', lang)}. ` +
        `${t('focusResearch', lang)}`);
    }

    if (sCurve.milestones.length > 0) {
      lines.push('');
      lines.push(`### ${t('keyEventsMilestones', lang)}`);
      lines.push('');
      for (const m of sCurve.milestones) {
        lines.push(`- **${m.year}** - ${m.label}: ${m.description}`);
      }
    }

    return lines.join('\n');
  }
}
