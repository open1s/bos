import { Agent, BrainOS } from '@open1s/ezbos';
import { SearchResult } from '../../domain/solution/search_port.js';
import { CachedSearchService } from '../../infrastructure/search/cached_search.js';
import { AISummarizer, SummarizationResult } from '../../infrastructure/search/ai_summarizer.js';
import { streamAgent } from '../../infrastructure/ai/streaming.js';
import {
  UnifiedResearchRequest,
  UnifiedResearchResult,
  ResearchError,
  ResearchMetadata,
  PriorArtItem,
} from './types.js';
import { UnifiedResearchService } from './service.js';
import { LocaleConfig, DEFAULT_LOCALE, t, stageLabel, trlTitle, svgLabel, getLanguagePrompt } from '../../domain/shared/i18n.js';

export interface AIResearchConfig {
  maxSearchResults?: number;
  onProgress?: (step: string, message: string) => void;
  onThinking?: (text: string) => void;
  showThinking?: boolean;
}

interface ExtractedParameters {
  improvingParameter?: string;
  worseningParameter?: string;
  technologyName?: string;
  performanceMetric?: string;
  keyInsights?: string[];
  recommendedApproach?: string;
}

interface ExtractedSearchKeywords {
  patentQuery: string;
  paperQuery: string;
  techQuery: string;
  reasoning: string;
}

export class AIResearchOrchestrator {
  private agent: Agent | null = null;
  private brain: BrainOS;
  private searchService: CachedSearchService;
  private summarizer: AISummarizer;
  private researchService: UnifiedResearchService;
  private errors: ResearchError[] = [];
  private metadata: Partial<ResearchMetadata> = {};
  private locale: LocaleConfig;

  constructor(
    brain: BrainOS,
    searchService: CachedSearchService,
    summarizer: AISummarizer,
    researchService: UnifiedResearchService,
    locale?: LocaleConfig,
  ) {
    this.brain = brain;
    this.searchService = searchService;
    this.summarizer = summarizer;
    this.researchService = researchService;
    this.locale = locale || DEFAULT_LOCALE;
  }

  async initialize(): Promise<void> {
    const langPrompt = getLanguagePrompt(this.locale.language);
    const builder = this.brain.agent('triz-research-orchestrator')
      .with_systemPrompt(`You are a TRIZ research expert. You analyze problems, search for prior art, summarize findings, and generate comprehensive research reports.

Your workflow:
1. Analyze the problem description
2. Search for patents, papers, and technical solutions
3. Summarize each result in context of the problem
4. Generate a comprehensive research report with:
   - Executive summary
   - Contradiction analysis (if applicable)
   - Prior art analysis with AI-generated summaries
   - Technology maturity assessment (TRL + S-curve)
   - Actionable recommendations

Return ONLY a JSON object with the report structure. No markdown, no explanation.

${langPrompt}`)
      .with_temperature(0.3);

    this.agent = await builder.start();
  }

  async research(
    problemDescription: string,
    config: AIResearchConfig = {},
  ): Promise<UnifiedResearchResult> {
    this.errors = [];
    this.metadata = {
      startedAt: Date.now(),
      sourcesUsed: [],
      cacheHits: 0,
      cacheMisses: 0,
      aiCallsMade: 0,
    };

    if (!this.agent) {
      try {
        await this.initialize();
      } catch (err) {
        this.addError('orchestrator', `Failed to initialize AI agent: ${err instanceof Error ? err.message : String(err)}`, 'error');
      }
    }

    const maxResults = config.maxSearchResults || 5;
    const onProgress = config.onProgress || (() => {});
    const onThinking = config.onThinking || (() => {});
    const showThinking = config.showThinking ?? true;

    // Step 1: Extract search keywords from problem description
    onProgress('keywords', 'AI is extracting optimized search keywords...');
    let searchKeywords: ExtractedSearchKeywords | null = null;
    if (this.agent) {
      try {
        const keywordPrompt = this.buildKeywordPrompt(problemDescription);
        const keywordResponse = await streamAgent(this.agent, keywordPrompt, {
          onThinking: (text) => {
            if (showThinking) {
              let buffer = '';
              buffer += text;
              if (buffer.length >= 150) {
                const flushAt = buffer.lastIndexOf(' ', 150);
                if (flushAt > 0) {
                  onThinking(buffer.slice(0, flushAt + 1));
                  buffer = buffer.slice(flushAt + 1);
                }
              }
            }
          },
        });
        this.metadata.aiCallsMade = (this.metadata.aiCallsMade || 0) + 1;
        searchKeywords = this.parseSearchKeywords(keywordResponse);
      } catch {
        // Fall back to raw problem description
      }
    }

    const patentQuery = searchKeywords?.patentQuery || problemDescription;
    const paperQuery = searchKeywords?.paperQuery || problemDescription;
    const techQuery = searchKeywords?.techQuery || problemDescription;

    if (searchKeywords && searchKeywords.patentQuery && searchKeywords.paperQuery && searchKeywords.techQuery) {
      onProgress('keywords', `Patent: "${patentQuery}" | Paper: "${paperQuery}" | Tech: "${techQuery}"`);
    }

    // Step 2: Search prior art with optimized keywords
    onProgress('search', 'Searching patents, papers, and technical solutions...');
    const [patents, papers, techSolutions] = await this.searchWithTracking(
      { patentQuery, paperQuery, techQuery },
      maxResults,
    );
    onProgress('search', `Found ${patents.length} patents, ${papers.length} papers, ${techSolutions.length} tech solutions`);

    if (patents.length === 0 && papers.length === 0 && techSolutions.length === 0) {
      this.addError('search', 'No prior art found. Results may be less reliable.', 'warning');
    }

    // Step 2: Summarize each result using AI (parallel)
    onProgress('summarize', 'AI is analyzing and summarizing each result...');
    const summarizedResults = await this.summarizeAllResultsParallel(
      { patents, papers, techSolutions },
      problemDescription,
      onProgress,
    );
    onProgress('summarize', 'Summarization complete');

    // Step 3: Build comprehensive prompt for AI analysis
    const analysisPrompt = this.buildAnalysisPrompt(
      problemDescription,
      summarizedResults,
    );

    // Step 4: Get AI analysis and report (streaming with thinking)
    onProgress('analyze', 'AI is extracting TRIZ parameters and analyzing contradictions...');
    let aiAnalysis: ExtractedParameters = {};
    if (this.agent) {
      try {
        let thinkingBuffer = '';
        const FLUSH_INTERVAL = 200; // characters
        const response = await streamAgent(this.agent, analysisPrompt, {
          onThinking: (text) => {
            if (!showThinking) return;
            thinkingBuffer += text;
            // Flush when buffer is large enough
            while (thinkingBuffer.length >= FLUSH_INTERVAL) {
              // Find last newline or space for clean break
              let flushAt = thinkingBuffer.lastIndexOf('\n', FLUSH_INTERVAL);
              if (flushAt === -1) flushAt = thinkingBuffer.lastIndexOf(' ', FLUSH_INTERVAL);
              if (flushAt === -1) flushAt = FLUSH_INTERVAL;

              const chunk = thinkingBuffer.slice(0, flushAt + 1);
              onThinking(chunk);
              thinkingBuffer = thinkingBuffer.slice(flushAt + 1);
            }
          },
          onToolCall: (name) => {
            onProgress('tool', `Calling tool: ${name}`);
          },
        });
        // Flush remaining thinking
        if (showThinking && thinkingBuffer.length > 0) {
          onThinking(thinkingBuffer);
        }
        this.metadata.aiCallsMade = (this.metadata.aiCallsMade || 0) + 1;
        aiAnalysis = this.parseAIAnalysisWithSchema(response);
      } catch (err) {
        this.addError('analyze', `AI analysis failed: ${err instanceof Error ? err.message : String(err)}`, 'warning');
      }
    }

    // Fallback: extract parameters if AI didn't provide them
    const fallbackParams = this.extractParametersFallback(problemDescription);
    aiAnalysis.improvingParameter = aiAnalysis.improvingParameter || fallbackParams.improvingParameter;
    aiAnalysis.worseningParameter = aiAnalysis.worseningParameter || fallbackParams.worseningParameter;
    aiAnalysis.technologyName = aiAnalysis.technologyName || fallbackParams.technologyName;
    aiAnalysis.performanceMetric = aiAnalysis.performanceMetric || fallbackParams.performanceMetric;
    onProgress('analyze', `Extracted: improving="${aiAnalysis.improvingParameter}", worsening="${aiAnalysis.worseningParameter}"`);

    // Step 5: Run TRIZ analysis (contradiction, S-curve, TRL)
    onProgress('triz', 'Running TRIZ contradiction matrix lookup, S-curve analysis, and TRL assessment...');
    let trizResult: UnifiedResearchResult | null = null;
    try {
      trizResult = await this.researchService.research({
        problemDescription,
        improvingParameter: aiAnalysis.improvingParameter,
        worseningParameter: aiAnalysis.worseningParameter,
        technologyName: aiAnalysis.technologyName,
        performanceMetric: aiAnalysis.performanceMetric,
        searchQuery: problemDescription,
        maxSearchResults: 0,
        onProgress: (step, message) => {
          onProgress(step, message);
        },
      });
    } catch (err) {
      this.addError('triz', `TRIZ analysis failed: ${err instanceof Error ? err.message : String(err)}`, 'warning');
    }
    onProgress('triz', `TRIZ analysis complete: ${trizResult?.contradictionAnalysis?.principles.length || 0} principles, TRL ${trizResult?.technologyMaturity?.trl.level || 'N/A'}`);

    // Collect errors from sub-service
    if (trizResult?.errors) {
      this.errors.push(...trizResult.errors);
    }

    // Step 6: Combine AI summaries with TRIZ analysis
    const finalReport = this.buildFinalReport(
      problemDescription,
      summarizedResults,
      trizResult,
      aiAnalysis,
      searchKeywords,
    );

    this.metadata.completedAt = Date.now();
    this.metadata.durationMs = this.metadata.completedAt - (this.metadata.startedAt || 0);

    return {
      summary: finalReport,
      contradictionAnalysis: trizResult?.contradictionAnalysis,
      priorArt: {
        patents: summarizedResults.patents,
        papers: summarizedResults.papers,
        techSolutions: summarizedResults.techSolutions,
      },
      technologyMaturity: trizResult?.technologyMaturity,
      recommendations: trizResult?.recommendations || [],
      errors: this.errors,
      metadata: this.metadata as ResearchMetadata,
    };
  }

  private async searchWithTracking(
    queries: { patentQuery: string; paperQuery: string; techQuery: string },
    maxResults: number,
  ): Promise<[SearchResult[], SearchResult[], SearchResult[]]> {
    const cache = this.searchService.getCache();
    const keys = [
      `patents:${queries.patentQuery}:${maxResults}`,
      `papers:${queries.paperQuery}:${maxResults}`,
      `tech:${queries.techQuery}:${maxResults}`,
    ];

    for (const key of keys) {
      if (cache.get(key)) {
        this.metadata.cacheHits = (this.metadata.cacheHits || 0) + 1;
      } else {
        this.metadata.cacheMisses = (this.metadata.cacheMisses || 0) + 1;
      }
    }

    const [patents, papers, techSolutions] = await Promise.all([
      this.searchService.searchPatents(queries.patentQuery, maxResults),
      this.searchService.searchPapers(queries.paperQuery, maxResults),
      this.searchService.searchTechSolutions(queries.techQuery, maxResults),
    ]);

    this.metadata.sourcesUsed = [
      patents.length > 0 ? 'patents' : '',
      papers.length > 0 ? 'papers' : '',
      techSolutions.length > 0 ? 'tech_solutions' : '',
    ].filter(Boolean);

    return [patents, papers, techSolutions];
  }

  private async summarizeAllResultsParallel(
    results: { patents: SearchResult[]; papers: SearchResult[]; techSolutions: SearchResult[] },
    problemDescription: string,
    onProgress: (step: string, message: string) => void,
  ): Promise<{
    patents: PriorArtItem[];
    papers: PriorArtItem[];
    techSolutions: PriorArtItem[];
  }> {
    const allItems = [
      ...results.patents.map((item, i) => ({ item, type: 'patent' as const, index: i })),
      ...results.papers.map((item, i) => ({ item, type: 'paper' as const, index: i })),
      ...results.techSolutions.map((item, i) => ({ item, type: 'tech_solution' as const, index: i })),
    ];

    const total = allItems.length;
    let completed = 0;

    const summarizeItem = async (
      entry: { item: SearchResult; type: 'patent' | 'paper' | 'tech_solution'; index: number },
    ): Promise<PriorArtItem> => {
      try {
        const summary = await this.summarizer.summarizeSnippet(
          entry.item.title,
          entry.item.snippet,
          problemDescription,
        );
        this.metadata.aiCallsMade = (this.metadata.aiCallsMade || 0) + 1;
        completed++;
        const typeLabel = entry.type === 'patent' ? 'Patent' : entry.type === 'paper' ? 'Paper' : 'Tech';
        const summaryPreview = summary.summary.slice(0, 120).replace(/\n/g, ' ');
        onProgress('summarize', `[${completed}/${total}] ${typeLabel}: ${entry.item.title.slice(0, 60)}... → ${summaryPreview}`);
        return {
          ...entry.item,
          summary,
          sourceType: entry.type,
          relevanceScore: this.calculateRelevance(summary, problemDescription),
        };
      } catch {
        completed++;
        return {
          ...entry.item,
          summary: undefined,
          sourceType: entry.type,
          relevanceScore: 0,
        };
      }
    };

    const summarized = await Promise.all(allItems.map(summarizeItem));

    const patents = summarized.filter((s): s is PriorArtItem => s.sourceType === 'patent');
    const papers = summarized.filter((s): s is PriorArtItem => s.sourceType === 'paper');
    const techSolutions = summarized.filter((s): s is PriorArtItem => s.sourceType === 'tech_solution');

    // Sort by relevance score descending
    patents.sort((a, b) => (b.relevanceScore || 0) - (a.relevanceScore || 0));
    papers.sort((a, b) => (b.relevanceScore || 0) - (a.relevanceScore || 0));
    techSolutions.sort((a, b) => (b.relevanceScore || 0) - (a.relevanceScore || 0));

    return { patents, papers, techSolutions };
  }

  private calculateRelevance(summary: SummarizationResult, problemDescription: string): number {
    const problemLower = problemDescription.toLowerCase();
    const problemKeywords = problemLower.split(/\s+/).filter(w => w.length > 3);

    let score = 0;
    const summaryText = `${summary.summary} ${summary.keyFindings.join(' ')} ${summary.relevanceToProblem}`.toLowerCase();

    for (const keyword of problemKeywords) {
      if (summaryText.includes(keyword)) score += 1;
    }

    // Bonus for high confidence
    if (summary.confidence && summary.confidence > 0.7) score += 2;

    // Penalty for low relevance statements
    if (summary.relevanceToProblem.toLowerCase().includes('low relevance')) score -= 3;
    if (summary.relevanceToProblem.toLowerCase().includes('not relevant')) score -= 5;

    return Math.max(0, score);
  }

  private buildAnalysisPrompt(
    problemDescription: string,
    results: { patents: PriorArtItem[]; papers: PriorArtItem[]; techSolutions: PriorArtItem[] },
  ): string {
    const formatResult = (r: PriorArtItem) => {
      const summaryText = r.summary
        ? `Summary: ${r.summary.summary}\nKey Findings: ${r.summary.keyFindings.join(', ')}\nRelevance: ${r.summary.relevanceToProblem}`
        : 'No summary available';
      return `Title: ${r.title}\nDate: ${r.publishedDate || 'N/A'}\nAuthors: ${r.authors?.join(', ') || 'N/A'}\n${summaryText}`;
    };

    return `Analyze this problem and prior art:

PROBLEM: ${problemDescription}

PRIOR ART:

PATENTS:
${results.patents.map((p, i) => `${i + 1}. ${formatResult(p)}`).join('\n\n')}

PAPERS:
${results.papers.map((p, i) => `${i + 1}. ${formatResult(p)}`).join('\n\n')}

TECH SOLUTIONS:
${results.techSolutions.map((t, i) => `${i + 1}. ${formatResult(t)}`).join('\n\n')}

Extract these fields. For improvingParameter and worseningParameter, use ONLY these exact names from the 39 TRIZ engineering parameters:
Weight of moving object, Weight of stationary object, Length of moving object, Length of stationary object, Area of moving object, Area of stationary object, Volume of moving object, Volume of stationary object, Speed, Force, Stress or pressure, Shape, Stability, Strength, Durability of moving object, Durability of stationary object, Temperature, Brightness, Energy spent by moving object, Energy spent by stationary object, Power, Loss of energy, Loss of substance, Loss of information, Loss of time, Amount of substance, Reliability, Measurement accuracy, Manufacturing precision, Harmful effects on object, Manufacturability, Convenience of use, Repairability, Adaptability, Complexity, Difficulty of detecting, Extent of automation, Productivity

Return ONLY valid JSON matching this schema:
{
  "improvingParameter": "EXACT parameter name from list above",
  "worseningParameter": "EXACT parameter name from list above",
  "technologyName": "string",
  "performanceMetric": "string",
  "keyInsights": ["string", "string", "string"],
  "recommendedApproach": "string"
}`;
  }

  private parseAIAnalysisWithSchema(response: string): ExtractedParameters {
    try {
      // Try to extract JSON from markdown code blocks first
      const codeBlockMatch = response.match(/```(?:json)?\s*([\s\S]*?)```/);
      if (codeBlockMatch) {
        const parsed = JSON.parse(codeBlockMatch[1].trim());
        return this.validateExtractedParameters(parsed);
      }

      // Try to find any JSON object
      const jsonMatch = response.match(/\{[\s\S]*\}/);
      if (jsonMatch) {
        const parsed = JSON.parse(jsonMatch[0]);
        return this.validateExtractedParameters(parsed);
      }
    } catch {
      // Will fall through to empty return
    }

    this.addError('parse', 'Failed to parse AI analysis response as JSON', 'warning');
    return {};
  }

  private validateExtractedParameters(parsed: unknown): ExtractedParameters {
    if (typeof parsed !== 'object' || parsed === null) {
      return {};
    }

    const result: ExtractedParameters = {};
    const obj = parsed as Record<string, unknown>;

    if (typeof obj.improvingParameter === 'string' && obj.improvingParameter.length > 0) {
      result.improvingParameter = obj.improvingParameter;
    }
    if (typeof obj.worseningParameter === 'string' && obj.worseningParameter.length > 0) {
      result.worseningParameter = obj.worseningParameter;
    }
    if (typeof obj.technologyName === 'string' && obj.technologyName.length > 0) {
      result.technologyName = obj.technologyName;
    }
    if (typeof obj.performanceMetric === 'string' && obj.performanceMetric.length > 0) {
      result.performanceMetric = obj.performanceMetric;
    }
    if (Array.isArray(obj.keyInsights) && obj.keyInsights.length > 0) {
      result.keyInsights = obj.keyInsights.filter((i): i is string => typeof i === 'string');
    }
    if (typeof obj.recommendedApproach === 'string' && obj.recommendedApproach.length > 0) {
      result.recommendedApproach = obj.recommendedApproach;
    }

    return result;
  }

  private buildKeywordPrompt(problemDescription: string): string {
    return `Extract optimized search keywords for prior art research based on this problem:

Problem: "${problemDescription}"

Generate THREE sets of search keywords optimized for different databases. For EACH database, provide:
- A primary query (5-10 core keywords)
- Alternative/synonym keywords (broader terms, related concepts, abbreviations)

Target databases:
1. patentQuery: For patent databases (Google Patents, USPTO, etc.) - use technical terms, mechanism-focused language
2. paperQuery: For academic databases (CrossRef, OpenAlex, etc.) - use academic terminology, research-focused language
3. techQuery: For technical articles and solutions - use industry terms, product names, practical language

Rules:
- Include synonyms, abbreviations, and related terms (e.g., "EV" + "electric vehicle" + "battery electric")
- Include broader terms that might capture relevant but not exact matches
- Use English terms even for non-English problems
- Separate keywords with spaces (for OR-style search)
- Don't use overly specific phrases - keep it broad enough to catch relevant results

Example format for each query:
"electric vehicle EV battery energy density range lightweight cost optimization materials solid-state lithium-ion"

Return ONLY valid JSON:
{
  "patentQuery": "keyword1 keyword2 synonym1 synonym2 ...",
  "paperQuery": "keyword1 keyword2 synonym1 synonym2 ...",
  "techQuery": "keyword1 keyword2 synonym1 synonym2 ...",
  "reasoning": "brief explanation of keyword choices"
}`;
  }

  private parseSearchKeywords(response: string): ExtractedSearchKeywords | null {
    try {
      const codeBlockMatch = response.match(/```(?:json)?\s*([\s\S]*?)```/);
      if (codeBlockMatch) {
        const parsed = JSON.parse(codeBlockMatch[1].trim());
        return this.validateSearchKeywords(parsed);
      }

      const jsonMatch = response.match(/\{[\s\S]*\}/);
      if (jsonMatch) {
        const parsed = JSON.parse(jsonMatch[0]);
        return this.validateSearchKeywords(parsed);
      }
    } catch {
      // Fall through
    }
    return null;
  }

  private validateSearchKeywords(parsed: unknown): ExtractedSearchKeywords | null {
    if (typeof parsed !== 'object' || parsed === null) return null;
    const obj = parsed as Record<string, unknown>;

    const patentQuery = typeof obj.patentQuery === 'string' ? obj.patentQuery.trim() : '';
    const paperQuery = typeof obj.paperQuery === 'string' ? obj.paperQuery.trim() : '';
    const techQuery = typeof obj.techQuery === 'string' ? obj.techQuery.trim() : '';
    const reasoning = typeof obj.reasoning === 'string' ? obj.reasoning : '';

    if (!patentQuery || !paperQuery || !techQuery) return null;

    return {
      patentQuery: patentQuery || '',
      paperQuery: paperQuery || '',
      techQuery: techQuery || '',
      reasoning,
    };
  }

  private extractParametersFallback(problemDescription: string): ExtractedParameters {
    const lower = problemDescription.toLowerCase();

    if (lower.includes('small') || lower.includes('size') || lower.includes('compact') || lower.includes('miniatur')) {
      return {
        improvingParameter: 'Size of moving object',
        worseningParameter: 'Loss of information',
        technologyName: 'Antenna Technology',
        performanceMetric: 'Signal range (km)',
      };
    }

    if (lower.includes('strength') || lower.includes('power') || lower.includes('durability')) {
      return {
        improvingParameter: 'Strength',
        worseningParameter: 'Weight',
        technologyName: 'Material Technology',
        performanceMetric: 'Strength-to-weight ratio',
      };
    }

    if (lower.includes('续航') || lower.includes('range') || lower.includes('duration') || lower.includes('speed')) {
      return {
        improvingParameter: 'Speed',
        worseningParameter: 'Weight',
        technologyName: 'Battery Technology',
        performanceMetric: 'Energy density (Wh/kg)',
      };
    }

    return {
      improvingParameter: 'Productivity',
      worseningParameter: 'Complexity',
      technologyName: 'System',
      performanceMetric: 'Performance',
    };
  }

  private buildFinalReport(
    problemDescription: string,
    results: { patents: PriorArtItem[]; papers: PriorArtItem[]; techSolutions: PriorArtItem[] },
    trizResult: UnifiedResearchResult | null,
    aiAnalysis: ExtractedParameters,
    searchKeywords: ExtractedSearchKeywords | null,
  ): string {
    const lang = this.locale.language;
    const lines: string[] = [];

    lines.push(`# ${t('title', lang)}`);
    lines.push('');
    lines.push(`**${t('problem', lang)}:** ${problemDescription}`);
    lines.push(`**${t('date', lang)}:** ${new Date().toISOString().split('T')[0]}`);
    lines.push('');

    // Search keywords
    if (searchKeywords) {
      lines.push(`## ${t('searchKeywords', lang)}`);
      lines.push('');
      lines.push(`| ${t('source', lang) || 'Source'} | Query |`);
      lines.push(`|--------|-------|`);
      lines.push(`| 🔍 ${t('patents', lang)} | \`${searchKeywords.patentQuery}\` |`);
      lines.push(`| 📚 ${t('academicPapersTitle', lang)} | \`${searchKeywords.paperQuery}\` |`);
      lines.push(`| 🔧 ${t('technicalSolutions', lang)} | \`${searchKeywords.techQuery}\` |`);
      if (searchKeywords.reasoning) {
        lines.push('');
        lines.push(`**${t('reasoning', lang) || 'Reasoning'}:** ${searchKeywords.reasoning}`);
      }
      lines.push('');
    }

    // Confidence banner
    const errorCount = this.errors.filter(e => e.severity === 'error').length;
    const warningCount = this.errors.filter(e => e.severity === 'warning').length;
    if (errorCount > 0 || warningCount > 0) {
      lines.push(`> ⚠️ **${t('analysisQuality', lang) || 'Analysis Quality'}:** ${errorCount} ${t('errors', lang) || 'error(s)'}，${warningCount} ${t('warnings', lang) || 'warning(s)'}。${t('reviewErrors', lang) || 'Review errors section for details.'}`);
      lines.push('');
    }

    // Executive Summary
    lines.push(`## ${t('executiveSummary', lang)}`);
    lines.push('');
    if (aiAnalysis.keyInsights && aiAnalysis.keyInsights.length > 0) {
      lines.push(`**${t('keyInsights', lang) || 'Key Insights'}:**`);
      lines.push('');
      for (const insight of aiAnalysis.keyInsights.slice(0, 3)) {
        lines.push(`- ${insight}`);
      }
      lines.push('');
    }
    if (aiAnalysis.recommendedApproach) {
      lines.push(`**${t('recommendedApproach', lang) || 'Recommended Approach'}:** ${aiAnalysis.recommendedApproach}`);
      lines.push('');
    }

    // Prior Art with AI Summaries (sorted by relevance)
    lines.push(`## ${t('priorArtAnalysis', lang)}`);
    lines.push('');

    const renderItems = (items: PriorArtItem[], category: string) => {
      if (items.length === 0) return;
      lines.push(`### ${category}（${items.length} ${t('found', lang) || 'found'}）`);
      lines.push('');
      for (const item of items) {
        const relevanceBadge = item.relevanceScore !== undefined
          ? ` [${t('relevance', lang) || 'Relevance'}: ${item.relevanceScore}]`
          : '';
        lines.push(`**${item.title}**${relevanceBadge}`);
        lines.push(`- **${t('date', lang)}:** ${item.publishedDate || 'N/A'}`);
        lines.push(`- **${t('authors', lang)}:** ${item.authors?.join(', ') || 'N/A'}`);
        if (item.summary) {
          lines.push(`- **${t('summary', lang)}:** ${item.summary.summary}`);
          lines.push(`- **${t('keyFindings', lang) || 'Key Findings'}:** ${item.summary.keyFindings.join('; ')}`);
          lines.push(`- **${t('relevance', lang) || 'Relevance'}:** ${item.summary.relevanceToProblem}`);
          if (item.summary.trizPrinciples.length > 0) {
            lines.push(`- **TRIZ ${t('principles', lang) || 'Principles'}:** ${item.summary.trizPrinciples.join(', ')}`);
          }
        } else {
          lines.push(`- **Summary:** _Not available_`);
        }
        lines.push(`- **URL:** ${item.url}`);
        lines.push('');
      }
    };

    renderItems(results.patents, 'Patents');
    renderItems(results.papers, 'Academic Papers');
    renderItems(results.techSolutions, t('technicalSolutions', lang));

    // TRIZ Analysis
    if (trizResult?.contradictionAnalysis) {
      lines.push(`## ${t('trizContradictionAnalysis', lang)}`);
      lines.push('');
      lines.push(`**${t('improving', lang)}:** ${trizResult.contradictionAnalysis.improvingParameter}`);
      lines.push(`**${t('worsening', lang)}:** ${trizResult.contradictionAnalysis.worseningParameter}`);
      lines.push('');
      lines.push(`**${t('recommendedPrinciples', lang)}:**`);
      lines.push('');
      for (const p of trizResult.contradictionAnalysis.principles) {
        lines.push(`- **#${p.index} ${p.name}**: ${p.description}`);
      }
      lines.push('');
    }

    // Technology Maturity
    if (trizResult?.technologyMaturity) {
      const { trl, trlNext, sCurveStage, sCurveStageNext, crossoverYear, sCurveData, svgPath, unicodeChart, milestones } = trizResult.technologyMaturity;
      lines.push(`## ${t('technologyMaturity', lang)}`);
      lines.push('');

      const estBadge = trl.isEstimated ? ` (_${t('aiEstimate', lang)}_)` : '';
      const dataBadge = sCurveData.isEstimated ? ` (_${t('aiEstimatedData', lang)}_)` : '';

      lines.push(`- **${t('sCurveStage', lang)}:** ${sCurveStage} → ${sCurveStageNext}${dataBadge}`);
      lines.push(`- **TRL:** ${trl.level}/9 - ${trlTitle(trl.level, lang)} (${Math.round(trl.confidence * 100)}% ${t('confidence', lang)})${estBadge}`);
      lines.push(`- **${t('nextGenTRL', lang)}:** ${trlNext.min}-${trlNext.max}/9`);
      lines.push(`- **${t('crossover', lang)}:** ~${crossoverYear}`);
      if (sCurveData.dataPointCount > 0) {
        lines.push(`- **${t('dataPoints', lang)}:** ${sCurveData.dataPointCount}${sCurveData.isEstimated ? ` (${t('estimated', lang)})` : ` (${t('real', lang)})`}`);
      }
      if (svgPath) {
        lines.push(`- **${t('scurveChart', lang)}:** \`${svgPath}\``);
      }
      lines.push('');

      if (unicodeChart) {
        lines.push(`### ${t('scurvePreview', lang)}`);
        lines.push('');
        lines.push('```');
        lines.push(unicodeChart);
        lines.push('```');
        lines.push('');
      }

      if (milestones && milestones.length > 0) {
        lines.push(`### ${t('keyEventsMilestones', lang)}`);
        lines.push('');
        for (const m of milestones) {
          lines.push(`- **${m.year}** - ${m.label}: ${m.description}`);
        }
        lines.push('');
      }
    }

    // Recommendations
    lines.push(`## ${t('recommendations', lang)}`);
    lines.push('');
    const recs = trizResult?.recommendations || [];
    for (const r of recs) {
      lines.push(`- ${r}`);
    }
    if (recs.length === 0) {
      lines.push(`- ${t('noRecommendations', lang)}`);
    }
    lines.push('');

    // Errors section
    if (this.errors.length > 0) {
      lines.push(`## ${t('analysisErrors', lang)}`);
      lines.push('');
      for (const err of this.errors) {
        const icon = err.severity === 'error' ? '❌' : '⚠️';
        lines.push(`- ${icon} **[${err.component}]** ${err.message}`);
      }
      lines.push('');
    }

    // Metadata
    if (this.metadata.completedAt) {
      lines.push(`## Research Metadata`);
      lines.push('');
      lines.push(`- **Duration:** ${Math.round((this.metadata.durationMs || 0) / 1000)}s`);
      lines.push(`- **Sources:** ${(this.metadata.sourcesUsed || []).join(', ') || 'none'}`);
      lines.push(`- **Cache:** ${this.metadata.cacheHits || 0} hits, ${this.metadata.cacheMisses || 0} misses`);
      lines.push(`- **AI Calls:** ${this.metadata.aiCallsMade || 0}`);
      lines.push('');
    }

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

  async close(): Promise<void> {
    if (this.agent) {
      await this.agent.close();
      this.agent = null;
    }
  }
}
