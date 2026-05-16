import { SearchResult } from '../../domain/solution/search_port.js';
import { CachedSearchService } from '../../infrastructure/search/cached_search.js';
import { ContradictionAnalysisService } from '../../domain/contradiction/services.js';
import { ContradictionMatrix } from '../../domain/contradiction/matrix.js';
import { PrincipleEngine } from '../../domain/principle/services.js';
import { AnalyzeSCurveHandler } from '../analyze_s_curve/handler.js';
import { TRLAssessor } from '../../infrastructure/triz/trl_assessor.js';
import { AiSCurveDataExtractor } from '../../infrastructure/s_curve/ai_data_extractor.js';
import {
  UnifiedResearchRequest,
  UnifiedResearchResult,
  ResearchError,
  PriorArtItem,
} from './types.js';
import { writeFileSync, mkdirSync, existsSync } from 'fs';
import { join, dirname } from 'path';
import { fileURLToPath } from 'url';
import { LocaleConfig, DEFAULT_LOCALE, t, stageLabel, stageStrategy, trlTitle } from '../../domain/shared/i18n.js';

export class UnifiedResearchService {
  private searchService: CachedSearchService;
  private contradictionService: ContradictionAnalysisService;
  private principleEngine: PrincipleEngine;
  private sCurveHandler: AnalyzeSCurveHandler;
  private trlAssessor: TRLAssessor;
  private dataExtractor: AiSCurveDataExtractor;
  private errors: ResearchError[] = [];
  private locale: LocaleConfig;

  constructor(deps: {
    searchService: CachedSearchService;
    contradictionService: ContradictionAnalysisService;
    principleEngine: PrincipleEngine;
    sCurveHandler: AnalyzeSCurveHandler;
    trlAssessor: TRLAssessor;
    dataExtractor: AiSCurveDataExtractor;
    locale?: LocaleConfig;
  }) {
    this.searchService = deps.searchService;
    this.contradictionService = deps.contradictionService;
    this.principleEngine = deps.principleEngine;
    this.sCurveHandler = deps.sCurveHandler;
    this.trlAssessor = deps.trlAssessor;
    this.dataExtractor = deps.dataExtractor;
    this.locale = deps.locale || DEFAULT_LOCALE;
  }

  async research(request: UnifiedResearchRequest): Promise<UnifiedResearchResult> {
    this.errors = [];
    const maxResults = request.maxSearchResults !== undefined ? request.maxSearchResults : 5;
    const searchQuery = request.searchQuery || request.problemDescription;
    const onProgress = request.onProgress || (() => {});

    // Step 1: Search prior art (real data from free APIs) - skip if maxResults is 0
    let patents: SearchResult[] = [];
    let papers: SearchResult[] = [];
    let techSolutions: SearchResult[] = [];

    if (maxResults > 0) {
      try {
        onProgress('search', 'Searching prior art databases...');
        [patents, papers, techSolutions] = await Promise.all([
          this.searchService.searchPatents(searchQuery, maxResults),
          this.searchService.searchPapers(searchQuery, maxResults),
          this.searchService.searchTechSolutions(searchQuery, maxResults),
        ]);
        onProgress('search', `Found ${patents.length} patents, ${papers.length} papers, ${techSolutions.length} tech solutions`);
      } catch (err) {
        this.addError('search', `Search failed: ${err instanceof Error ? err.message : String(err)}`, 'warning');
      }
    }

    // Step 2: Analyze contradiction if parameters provided
    let contradictionAnalysis;
    if (request.improvingParameter && request.worseningParameter) {
      const improvingParam = this.resolveParameter(request.improvingParameter);
      const worseningParam = this.resolveParameter(request.worseningParameter);

      if (improvingParam && worseningParam) {
        try {
          onProgress('contradiction', `Looking up matrix: ${request.improvingParameter} vs ${request.worseningParameter}...`);
          const result = this.contradictionService.analyze(
            improvingParam,
            worseningParam,
            request.problemDescription,
          );
          contradictionAnalysis = {
            improvingParameter: request.improvingParameter,
            worseningParameter: request.worseningParameter,
            contradictionId: result.contradiction.id,
            principles: result.principles.map(p => ({
              index: p.index,
              name: p.name,
              description: p.description,
            })),
          };
          onProgress('contradiction', `Found ${result.principles.length} principles: ${result.principles.slice(0, 3).map(p => `#${p.index} ${p.name}`).join(', ')}`);
        } catch (err) {
          this.addError('contradiction', `Contradiction analysis failed: ${err instanceof Error ? err.message : String(err)}`, 'warning');
        }
      } else {
        this.addError('contradiction', `Could not resolve parameters: improving="${request.improvingParameter}", worsening="${request.worseningParameter}"`, 'warning');
      }
    }

    // Step 3: Analyze S-curve and TRL if technology provided
    let technologyMaturity;
    if (request.technologyName && request.performanceMetric) {
      try {
        onProgress('s_curve', `Extracting S-curve data for ${request.technologyName} (${request.performanceMetric})...`);
        const extracted = await this.dataExtractor.extractData(
          request.technologyName,
          request.performanceMetric,
        );

        const hasRealData = extracted.dataPoints && extracted.dataPoints.length > 0;
        const isEstimated = !hasRealData;
        onProgress('s_curve', `Data points: ${extracted.dataPoints?.length || 0} (${isEstimated ? 'AI-estimated' : 'real'})`);

        if (!hasRealData) {
          this.addError('s_curve', 'No real S-curve data found. Using AI-estimated data points. Results are approximate.', 'warning');
        }

        onProgress('s_curve', `Fitting logistic curve and detecting stage...`);
        const sCurveResult = await this.sCurveHandler.execute({
          technologyName: request.technologyName,
          performanceMetric: request.performanceMetric,
          dataPoints: extracted.dataPoints || [],
          milestones: extracted.milestones || [],
        });
        onProgress('s_curve', `S1 stage: ${sCurveResult.s1Stage}, S2 stage: ${sCurveResult.s2Stage}, crossover: ~${sCurveResult.crossoverYear}`);

        // Save SVG to file
        const svgPath = this.saveSvgToFile(sCurveResult.svg, request.technologyName);
        onProgress('s_curve', `SVG saved to: ${svgPath}`);

        if (sCurveResult.s1TRL && sCurveResult.s2TRLRange) {
          technologyMaturity = {
            sCurveStage: sCurveResult.s1Stage,
            sCurveStageNext: sCurveResult.s2Stage,
            crossoverYear: sCurveResult.crossoverYear,
            trl: {
              level: sCurveResult.s1TRL.level,
              title: sCurveResult.s1TRL.title,
              confidence: sCurveResult.s1TRL.confidence,
              isEstimated,
            },
            trlNext: {
              min: sCurveResult.s2TRLRange.min,
              max: sCurveResult.s2TRLRange.max,
              mostLikely: sCurveResult.s2TRLRange.mostLikely,
            },
            sCurveData: {
              isEstimated,
              dataPointCount: extracted.dataPoints?.length || 0,
              confidence: isEstimated ? 0.3 : 0.8,
            },
            svgPath,
            unicodeChart: sCurveResult.unicodeChart,
            milestones: sCurveResult.milestones,
          };
        }
      } catch (err) {
        this.addError('s_curve', `S-curve/TRL analysis failed: ${err instanceof Error ? err.message : String(err)}`, 'warning');
      }
    }

    // Build prior art items
    const toPriorArt = (items: SearchResult[], type: 'patent' | 'paper' | 'tech_solution'): PriorArtItem[] =>
      items.map(item => ({ ...item, sourceType: type }));

    // Step 4: Generate recommendations
    const recommendations = this.generateRecommendations({
      contradictionAnalysis,
      priorArt: { patents, papers, techSolutions },
      technologyMaturity,
      problemDescription: request.problemDescription,
    });

    // Step 5: Build comprehensive report
    const summary = this.buildReport(request, {
      contradictionAnalysis,
      priorArt: { patents, papers, techSolutions },
      technologyMaturity,
    });

    return {
      summary,
      contradictionAnalysis,
      priorArt: {
        patents: toPriorArt(patents, 'patent'),
        papers: toPriorArt(papers, 'paper'),
        techSolutions: toPriorArt(techSolutions, 'tech_solution'),
      },
      technologyMaturity,
      recommendations,
      errors: this.errors,
    };
  }

  private resolveParameter(param: unknown): number | null {
    if (!param) return null;

    if (typeof param === 'number') {
      return param >= 1 && param <= 39 ? param : null;
    }

    if (typeof param === 'string') {
      // Try parsing as number first
      const num = parseInt(param, 10);
      if (!isNaN(num) && num >= 1 && num <= 39) return num;

      // Handle comma-separated parameters (take the first match)
      const parts = param.split(',').map(p => p.trim());
      for (const part of parts) {
        const resolved = this.resolveSingleParameter(part);
        if (resolved !== null) return resolved;
      }
    }

    if (Array.isArray(param) && param.length > 0) {
      return this.resolveParameter(param[0]);
    }

    return null;
  }

  private resolveSingleParameter(input: string): number | null {
    const matrix = ContradictionMatrix.getInstance();
    const allParams = matrix.getAllParameters();
    const lower = input.toLowerCase().trim();

    // Clean up common AI response patterns
    const cleaned = lower
      .replace(/^\d+[\.\)\-]\s*/, '')  // Remove leading "1. ", "2) ", etc.
      .replace(/\(.*?\)/g, '')          // Remove parenthetical notes
      .replace(/\s+/g, ' ')             // Normalize whitespace
      .trim();

    // Exact match
    for (const p of allParams) {
      if (p.name.toLowerCase() === cleaned) return p.index;
    }

    // Parameter name contains the input
    for (const p of allParams) {
      if (p.name.toLowerCase().includes(cleaned)) return p.index;
    }

    // Input contains parameter name
    for (const p of allParams) {
      if (cleaned.includes(p.name.toLowerCase())) return p.index;
    }

    // Keyword-based matching for common AI responses
    const keywordMap: Record<string, number> = {
      'weight': 1,
      'speed': 9,
      'force': 10,
      'strength': 14,
      'stability': 13,
      'temperature': 17,
      'power': 21,
      'energy': 19,
      'time': 25,
      'reliability': 27,
      'accuracy': 28,
      'productivity': 39,
      'complexity': 36,
      'automation': 38,
      'range': 9,
      'duration': 15,
      'cost': 39,
      'volume': 8,
      'area': 6,
      'length': 4,
      'size': 4,
      'shape': 12,
      'brightness': 18,
      'manufactur': 32,
      'repair': 33,
      'adapt': 35,
      'detect': 37,
    };

    for (const [keyword, index] of Object.entries(keywordMap)) {
      if (cleaned.includes(keyword)) return index;
    }

    return null;
  }

  private generateRecommendations(result: {
    contradictionAnalysis?: { principles: Array<{ index: number; name: string }> };
    priorArt: { patents: SearchResult[]; papers: SearchResult[]; techSolutions: SearchResult[] };
    technologyMaturity?: { trl: { level: number }; sCurveStage: string };
    problemDescription: string;
  }): string[] {
    const lang = this.locale.language;
    const recs: string[] = [];

    if (result.contradictionAnalysis) {
      const principles = result.contradictionAnalysis.principles.slice(0, 3);
      recs.push(`${t('applyTrizPrinciples', lang)}: ${principles.map(p => `#${p.index} ${p.name}`).join(', ')}`);
    }

    if (result.priorArt.patents.length > 0) {
      recs.push(`${t('reviewRelevantPatents', lang)} ${result.priorArt.patents.length} ${t('relevantPatents', lang)}`);
    }

    if (result.priorArt.papers.length > 0) {
      recs.push(`${t('studyRelevantPapers', lang)} ${result.priorArt.papers.length} ${t('relevantPapers', lang)}`);
    }

    if (result.technologyMaturity) {
      const { trl, sCurveStage } = result.technologyMaturity;
      if (trl.level >= 7) {
        recs.push(`${t('techMature', lang)}（TRL ${trl.level}，${sCurveStage}）`);
      } else if (trl.level >= 4) {
        recs.push(`${t('techDeveloping', lang)}（TRL ${trl.level}）。${t('acceleratePrototyping', lang) || 'Accelerate prototyping and validation'}`);
      } else {
        recs.push(`${t('techEarly', lang)}（TRL ${trl.level}）。${t('investResearch', lang) || 'Invest in fundamental research'}`);
      }
    }

    return recs;
  }

  private buildReport(request: UnifiedResearchRequest, result: {
    contradictionAnalysis?: {
      improvingParameter: unknown;
      worseningParameter: unknown;
      principles: Array<{ index: number; name: string; description: string }>;
    };
    priorArt: { patents: SearchResult[]; papers: SearchResult[]; techSolutions: SearchResult[] };
    technologyMaturity?: {
      trl: { level: number; title: string; confidence: number; isEstimated?: boolean };
      trlNext: { min: number; max: number; mostLikely: number };
      sCurveStage: string;
      sCurveStageNext: string;
      crossoverYear: number;
      sCurveData?: { isEstimated: boolean; dataPointCount: number };
      svgPath?: string;
      unicodeChart?: string;
      milestones?: Array<{ year: number; label: string; description: string; type: string }>;
    };
  }): string {
    const lang = this.locale.language;
    const lines: string[] = [];

    lines.push(`# ${t('title', lang)}`);
    lines.push('');
    lines.push(`**${t('problem', lang)}:** ${request.problemDescription}`);
    lines.push(`**${t('date', lang)}:** ${new Date().toISOString().split('T')[0]}`);
    lines.push('');

    // Executive Summary
    lines.push(`## ${t('executiveSummary', lang)}`);
    lines.push('');
    const totalPriorArt = result.priorArt.patents.length + result.priorArt.papers.length + result.priorArt.techSolutions.length;
    if (result.contradictionAnalysis) {
      lines.push(`本分析识别出 **${result.contradictionAnalysis.principles.length} 个TRIZ发明原理** 可以解决你问题中的核心矛盾。` +
        `我们找到了 **${totalPriorArt} 个相关现有技术项**（${result.priorArt.patents.length} 项专利，${result.priorArt.papers.length} 篇论文，${result.priorArt.techSolutions.length} 个技术方案）。`);
    } else {
      lines.push(`我们找到了 **${totalPriorArt} 个相关现有技术项**（${result.priorArt.patents.length} 项专利，${result.priorArt.papers.length} 篇论文，${result.priorArt.techSolutions.length} 个技术方案）与你的问题相关。`);
    }

    if (result.technologyMaturity) {
      const { trl, sCurveStage, crossoverYear } = result.technologyMaturity;
      const estBadge = trl.isEstimated ? ` (_${t('aiEstimate', lang)}_)` : '';
      lines.push(`该技术目前处于 **TRL ${trl.level}/9** (${trlTitle(trl.level, lang)})${estBadge}，S曲线阶段为 **${sCurveStage}**，` +
        `下一代解决方案预计在 **${crossoverYear}** 年左右超越当前性能。`);
    }
    lines.push('');

    // Contradiction Analysis
    if (result.contradictionAnalysis) {
      lines.push(`## 1. ${t('contradictionAnalysis', lang)}`);
      lines.push('');
      lines.push(`**${t('improvingParameter', lang)}:** ${result.contradictionAnalysis.improvingParameter}`);
      lines.push(`**${t('worseningParameter', lang)}:** ${result.contradictionAnalysis.worseningParameter}`);
      lines.push('');
      const topPrinciples = result.contradictionAnalysis.principles.slice(0, 3);
      lines.push(`核心矛盾可以通过 **${result.contradictionAnalysis.principles.length} 个TRIZ发明原理** 解决：`);
      lines.push('');
      for (let i = 0; i < topPrinciples.length; i++) {
        const p = topPrinciples[i];
        lines.push(`${i + 1}. **原理 #${p.index}: ${p.name}** - ${p.description.slice(0, 120)}...`);
      }
      lines.push('');
    }

    // Prior Art Analysis
    lines.push(`## 2. ${t('priorArtAnalysis', lang)}`);
    lines.push('');

    if (result.priorArt.patents.length > 0) {
      lines.push(`### ${t('patentLandscape', lang)}（${result.priorArt.patents.length} ${t('found', lang) || 'found'}）`);
      lines.push('');
      lines.push(`**${t('summary', lang)}:** ${t('patentLandscapeSummary', lang)} ${result.priorArt.patents.length} ${t('relevantPatents', lang) || 'relevant patents'}。` +
        `${t('keyPlayers', lang)} ${this.extractTopAuthors(result.priorArt.patents)}。` +
        `${t('patentsSpan', lang)} ${this.extractDateRange(result.priorArt.patents)}，${t('indicating', lang)} ${this.getPatentTrend(result.priorArt.patents)}。`);
      lines.push('');

      lines.push(`| # | ${t('title', lang)} | ${t('date', lang)} | ${t('authors', lang) || 'Authors'} |`);
      lines.push(`|---|-------|------|---------|`);
      for (let i = 0; i < result.priorArt.patents.length; i++) {
        const p = result.priorArt.patents[i];
        lines.push(`| ${i + 1} | ${p.title.slice(0, 50)}${p.title.length > 50 ? '...' : ''} | ${p.publishedDate || 'N/A'} | ${p.authors?.slice(0, 2).join(', ') || 'N/A'} |`);
      }
      lines.push('');
      lines.push(`**${t('keyInsight', lang)}:** ${this.getPatentInsight(result.priorArt.patents)}`);
      lines.push('');
    }

    if (result.priorArt.papers.length > 0) {
      lines.push(`### ${t('academicResearch', lang)}（${result.priorArt.papers.length} ${t('found', lang) || 'found'}）`);
      lines.push('');
      lines.push(`**${t('summary', lang)}:** ${t('academicSummary', lang) || 'Academic research provides'} ${result.priorArt.papers.length} ${t('relevantPapers', lang) || 'relevant papers'}。` +
        `${t('recentWork', lang)} ${this.extractTopAuthors(result.priorArt.papers)} ${t('demonstrates', lang)} ${this.getResearchTrend(result.priorArt.papers)}。`);
      lines.push('');

      lines.push(`| # | ${t('title', lang)} | ${t('year', lang)} | ${t('authors', lang) || 'Authors'} |`);
      lines.push(`|---|-------|------|---------|`);
      for (let i = 0; i < result.priorArt.papers.length; i++) {
        const p = result.priorArt.papers[i];
        lines.push(`| ${i + 1} | ${p.title.slice(0, 50)}${p.title.length > 50 ? '...' : ''} | ${p.publishedDate || 'N/A'} | ${p.authors?.slice(0, 2).join(', ') || 'N/A'} |`);
      }
      lines.push('');
      lines.push(`**${t('keyInsight', lang)}:** ${this.getResearchInsight(result.priorArt.papers)}`);
      lines.push('');
    }

    if (result.priorArt.techSolutions.length > 0) {
      lines.push(`### ${t('techSolutions', lang)}（${result.priorArt.techSolutions.length} ${t('found', lang) || 'found'}）`);
      lines.push('');
      lines.push(`**${t('summary', lang)}:** ${result.priorArt.techSolutions.length} ${t('techSolutionSummary', lang) || 'practical implementations demonstrate'} TRIZ原理的实际应用。` +
        `${t('show', lang)} ${this.getTechSolutionTrend(result.priorArt.techSolutions)}。`);
      lines.push('');

      lines.push(`| # | ${t('title', lang)} | ${t('date', lang)} |`);
      lines.push(`|---|-------|------|`);
      for (let i = 0; i < result.priorArt.techSolutions.length; i++) {
        const item = result.priorArt.techSolutions[i];
        lines.push(`| ${i + 1} | ${item.title.slice(0, 50)}${item.title.length > 50 ? '...' : ''} | ${item.publishedDate || 'N/A'} |`);
      }
      lines.push('');
      lines.push(`**${t('keyInsight', lang)}:** ${this.getTechSolutionInsight(result.priorArt.techSolutions)}`);
      lines.push('');
    }

    // Technology Maturity
    if (result.technologyMaturity) {
      const { trl, trlNext, sCurveStage, sCurveStageNext, crossoverYear, sCurveData, svgPath, unicodeChart, milestones } = result.technologyMaturity;
      lines.push(`## 3. ${t('technologyMaturity', lang)}`);
      lines.push('');

      const estBadge = trl.isEstimated ? ` (_${t('aiEstimate', lang)}_)` : '';
      const dataBadge = sCurveData?.isEstimated ? ` (_${t('aiEstimatedData', lang)}_)` : '';

      lines.push(`### ${t('summary', lang)}`);
      lines.push('');
      lines.push(`该技术目前处于 **TRL ${trl.level}/9** (${trlTitle(trl.level, lang)})${estBadge}，S曲线阶段为 **${sCurveStage}**${dataBadge}。` +
        `这表明 ${this.getMaturitySummary(sCurveStage, trl.level)}。` +
        `下一代技术预计在未来几年内达到 TRL ${trlNext.mostLikely}，` +
        `预测交叉点在 **${crossoverYear}** 年左右。`);
      lines.push('');

      lines.push(`### ${t('currentTechnology', lang)}`);
      lines.push('');
      lines.push(`| ${t('metric', lang)} | ${t('value', lang)} |`);
      lines.push(`|--------|-------|`);
      lines.push(`| **${t('sCurveStage', lang)}** | ${sCurveStage} |`);
      lines.push(`| **TRL** | ${trl.level}/9 - ${trlTitle(trl.level, lang)} |`);
      lines.push(`| **${t('confidence', lang)}** | ${Math.round(trl.confidence * 100)}% |`);
      if (sCurveData?.dataPointCount) {
        lines.push(`| **${t('dataPoints', lang)}** | ${sCurveData.dataPointCount}${sCurveData.isEstimated ? ` (${t('estimated', lang)})` : ` (${t('real', lang)})` } |`);
      }
      lines.push('');

      lines.push(`**${t('strategy', lang)}:** ${stageStrategy(sCurveStage, lang)}`);
      lines.push('');

      lines.push(`### ${t('nextGenTechnology', lang)}`);
      lines.push('');
      lines.push(`| ${t('metric', lang)} | ${t('value', lang)} |`);
      lines.push(`|--------|-------|`);
      lines.push(`| **${t('sCurveStage', lang)}** | ${sCurveStageNext} |`);
      lines.push(`| **TRL ${t('value', lang) || 'Range'}** | ${trlNext.min}-${trlNext.max}/9 (${t('mostLikely', lang)}: ${trlNext.mostLikely}) |`);
      lines.push(`| **${t('sCurveCrossover', lang)}** | ~${crossoverYear} |`);
      lines.push('');

      const yearsToCrossover = crossoverYear - new Date().getFullYear();
      if (yearsToCrossover > 0) {
        lines.push(`**${t('strategicWarning', lang)}:** ${t('willSurpass', lang)} **${yearsToCrossover} ${t('years', lang)}**。` +
          `${t('beginInvesting', lang)}`);
      } else {
        lines.push(`**${t('criticalAlert', lang)}:** ${t('hasSurpassed', lang)}` +
          `${t('immediateTransition', lang)}`);
      }
      lines.push('');

      // S-Curve Visualization
      if (result.technologyMaturity.svgPath) {
        lines.push(`### ${t('scurveVisualization', lang)}`);
        lines.push('');
        lines.push(`**${t('svgChart', lang)}:** \`${result.technologyMaturity.svgPath}\``);
        lines.push('');
        if (result.technologyMaturity.unicodeChart) {
          lines.push(`**${t('asciiPreview', lang)}:**`);
          lines.push('```');
          lines.push(result.technologyMaturity.unicodeChart);
          lines.push('```');
          lines.push('');
        }
      }

      // Key Events / Milestones
      if (result.technologyMaturity.milestones && result.technologyMaturity.milestones.length > 0) {
        lines.push(`### ${t('keyEventsMilestones', lang)}`);
        lines.push('');
        lines.push(`| ${t('year', lang)} | ${t('event', lang)} | ${t('type', lang)} |`);
        lines.push(`|------|-------|------|`);
        for (const m of result.technologyMaturity.milestones) {
          lines.push(`| ${m.year} | ${m.label} - ${m.description.slice(0, 60)}${m.description.length > 60 ? '...' : ''} | ${m.type} |`);
        }
        lines.push('');
      }
    }

    // Recommendations
    lines.push(`## 4. ${t('recommendations', lang)}`);
    lines.push('');

    if (result.contradictionAnalysis && result.contradictionAnalysis.principles.length > 0) {
      const topPrinciples = result.contradictionAnalysis.principles.slice(0, 3);
      lines.push(`### ${t('immediateActions', lang)}`);
      lines.push('');
      for (let i = 0; i < topPrinciples.length; i++) {
        const p = topPrinciples[i];
        lines.push(`${i + 1}. **${t('applyPrinciple', lang)} #${p.index} (${p.name})**: ${p.description.slice(0, 100)}...`);
      }
      lines.push('');
    }

    lines.push(`### ${t('researchPriorities', lang)}`);
    lines.push('');
    lines.push(`1. **${t('patentLandscape', lang)}**: ${t('reviewPatents', lang)} ${result.priorArt.patents.length} ${t('identifiedPatents', lang)}`);
    lines.push(`2. **${t('academicResearch', lang)}**: ${t('studyPapers', lang)} ${result.priorArt.papers.length} ${t('academicPapers', lang)}`);
    lines.push(`3. **${t('techRoadmap', lang) || '技术路线图'}**: ${t('techRoadmap', lang)}`);
    lines.push('');

    return lines.join('\n');
  }

  private addError(component: string, message: string, severity: 'warning' | 'error'): void {
    this.errors.push({
      component,
      message,
      severity,
      timestamp: Date.now(),
    });
  }

  private getContradictionInsight(principles: Array<{ name: string }>): string {
    if (principles.length === 0) return 'applying systematic innovation methods';
    const names = principles.slice(0, 2).map(p => p.name.toLowerCase());
    if (names.some(n => n.includes('segment'))) return 'breaking the system into smaller, more manageable components';
    if (names.some(n => n.includes('dynam'))) return 'making the system adaptable to changing conditions';
    if (names.some(n => n.includes('composite'))) return 'using composite materials or structures';
    return 'combining multiple inventive approaches to resolve the contradiction';
  }

  private extractTopAuthors(results: SearchResult[]): string {
    const authors = results
      .flatMap(r => r.authors || [])
      .filter(Boolean)
      .slice(0, 3);
    return authors.length > 0 ? authors.join(', ') : 'multiple research teams';
  }

  private extractDateRange(results: SearchResult[]): string {
    const dates = results
      .map(r => r.publishedDate)
      .filter(Boolean)
      .sort();
    if (dates.length === 0) return '不同时期';
    if (dates.length === 1) return dates[0];
    return `${dates[0]} 至 ${dates[dates.length - 1]}`;
  }

  private getPatentTrend(results: SearchResult[]): string {
    const dates = results.map(r => r.publishedDate).filter(Boolean);
    if (dates.length === 0) return '持续的研究活动';
    const recent = dates.filter(d => {
      const year = parseInt(d.slice(0, 4));
      return year > 2020;
    }).length;
    if (recent > dates.length / 2) return '该领域创新活跃且不断增长';
    return '成熟但不断发展的创新格局';
  }

  private getPatentInsight(_results: SearchResult[]): string {
    return `这些专利代表了当前的技术水平。审查它们以识别空白机会并避免侵权。` +
      `考虑在现有现有技术未覆盖的领域申请专利。`;
  }

  private getResearchTrend(_results: SearchResult[]): string {
    return t('researchTrend', this.locale.language);
  }

  private getResearchInsight(_results: SearchResult[]): string {
    return t('researchInsight', this.locale.language);
  }

  private getTechSolutionTrend(_results: SearchResult[]): string {
    return t('techSolutionTrend', this.locale.language);
  }

  private getTechSolutionInsight(_results: SearchResult[]): string {
    return t('techSolutionInsight', this.locale.language);
  }

  private getMaturitySummary(stage: string, trl: number): string {
    const lang = this.locale.language;
    if (trl >= 8) return t('maturityHigh', lang);
    if (trl >= 6) return t('maturityMid', lang);
    if (trl >= 4) return t('maturityLow', lang);
    return t('maturityEarly', lang);
  }

  private saveSvgToFile(svg: string, technologyName: string): string {
    const __dirname = dirname(fileURLToPath(import.meta.url));
    const outputDir = join(__dirname, '..', '..', '..', 'output');

    if (!existsSync(outputDir)) {
      mkdirSync(outputDir, { recursive: true });
    }

    const timestamp = new Date().toISOString().replace(/[:.]/g, '-').slice(0, 19);
    const sanitizedName = technologyName.replace(/[^a-zA-Z0-9]/g, '_').slice(0, 30);
    const filename = `scurve_${sanitizedName}_${timestamp}.svg`;
    const filePath = join(outputDir, filename);

    writeFileSync(filePath, svg, 'utf-8');
    return filePath;
  }

  private getStageStrategy(stage: string): string {
    return stageStrategy(stage, this.locale.language);
  }
}
