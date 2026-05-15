import { Agent, BrainOS } from '@open1s/ezbos';
import { streamAgent } from '../ai/streaming.js';
import { CachedSearchService } from '../search/cached_search.js';
import { SearchResult } from '../../domain/solution/search_port.js';
import { Milestone } from '../../domain/s_curve/value_objects.js';
import { LocaleConfig, DEFAULT_LOCALE, getLanguagePrompt } from '../../domain/shared/i18n.js';

export interface ExtractedDataPoint {
  x: number;
  y: number;
  source?: string;
}

export interface AiSCurveDataResult {
  dataPoints: ExtractedDataPoint[];
  milestones: Milestone[];
  sources: string[];
  reasoning: string;
}

export class AiSCurveDataExtractor {
  private agent: Agent | null = null;
  private brain: BrainOS | null = null;
  private searchService: CachedSearchService;
  private locale: LocaleConfig;

  constructor(searchService: CachedSearchService, brain?: BrainOS, locale?: LocaleConfig) {
    this.searchService = searchService;
    this.brain = brain || null;
    this.locale = locale || DEFAULT_LOCALE;
  }

  async initialize(): Promise<void> {
    if (!this.brain) {
      this.brain = new BrainOS();
      await this.brain.start();
    }

    const builder = this.brain.agent('triz-scurve-data-extractor')
      .with_systemPrompt(`You are a TRIZ S-Curve data extraction expert. Your task is to provide historical performance data that covers the FULL technology lifecycle.

Given a technology name and performance metric, provide realistic historical data points that span from the technology's INCEPTION to the PRESENT DAY.

CRITICAL: You MUST provide data points covering ALL stages of the S-curve lifecycle:
- INFANCY: Early development, slow initial growth (first data point should be from the technology's invention year)
- GROWTH: Rapid improvement phase (multiple data points showing acceleration)
- MATURITY: Slowing growth, approaching limits (recent years showing diminishing returns)
- DECLINE: Near ceiling, being replaced (projected near-future if applicable)

Return ONLY a JSON object with:
{
  "dataPoints": [{"x": year, "y": performance_value, "stage": "infancy|growth|maturity|decline"}],
  "milestones": [{"year": number, "label": "short title", "description": "brief description", "type": "invention|breakthrough|commercialization|standardization|peak|decline"}],
  "sources": ["list of typical sources for this data"],
  "reasoning": "brief explanation of data sources, accuracy, and lifecycle coverage",
  "lifecycleInfo": {
    "inventionYear": number,
    "growthStartYear": number,
    "maturityStartYear": number,
    "currentYear": number
  }
}

Provide 8-12 data points and 4-8 key milestones spanning the FULL lifecycle from invention to present. Be realistic with numbers. The data should show a clear S-shaped curve pattern.

${getLanguagePrompt(this.locale.language)}`)
      .with_temperature(0.1);

    this.agent = await builder.start();
  }

  async extractData(
    technologyName: string,
    performanceMetric: string,
  ): Promise<AiSCurveDataResult> {
    if (!this.agent) await this.initialize();

    const searchQuery = `${technologyName} ${performanceMetric} historical data performance evolution`;

    const searchResults = await this.searchService.searchTechSolutions(searchQuery, 10);

    const snippets = searchResults.length > 0
      ? searchResults
          .map(r => `Title: ${r.title}\nSnippet: ${r.snippet}\nURL: ${r.url}\nDate: ${r.publishedDate || 'unknown'}`)
          .join('\n\n')
      : 'No search results available. Use your domain knowledge to provide realistic data points.';

    const prompt = `Extract or estimate historical performance data points for ${technologyName} measured in ${performanceMetric}.

IMPORTANT: Provide data covering the FULL technology lifecycle from INVENTION to PRESENT (2026).
Include data points for: Infancy (early slow growth) → Growth (rapid acceleration) → Maturity (slowing) → Decline (near ceiling).

Also identify 4-8 KEY MILESTONES/EVENTS in the technology's history, such as:
- Invention/discovery year
- Major breakthroughs
- First commercial product
- Industry standards established
- Performance peak
- Signs of decline/replacement

Search results:
${snippets}

Return JSON with:
- dataPoints: 8-12 points spanning full lifecycle (x=year, y=performance value, stage="infancy|growth|maturity|decline")
- milestones: 4-8 key events (year, label, description, type="invention|breakthrough|commercialization|standardization|peak|decline")
- sources: list of typical sources
- reasoning: explanation of data and lifecycle coverage
- lifecycleInfo: {inventionYear, growthStartYear, maturityStartYear, currentYear}

${getLanguagePrompt(this.locale.language)}`;

    const response = await streamAgent(this.agent!, prompt);
    return this.parseResponse(response, searchResults);
  }

  private parseResponse(response: string, searchResults: SearchResult[]): AiSCurveDataResult {
    try {
      const jsonMatch = response.match(/\{[\s\S]*\}/);
      if (jsonMatch) {
        const parsed = JSON.parse(jsonMatch[0]);
        const dataPoints = (parsed.dataPoints || []).map((dp: any) => ({
          x: dp.x,
          y: dp.y,
          source: dp.source,
        }));

        const milestones: Milestone[] = (parsed.milestones || []).map((m: any) => ({
          year: m.year,
          label: m.label,
          description: m.description,
          type: m.type as Milestone['type'],
        }));

        // If no data points returned, generate lifecycle-spanning defaults
        if (dataPoints.length === 0) {
          return this.generateLifecycleDefaults(searchResults);
        }

        return {
          dataPoints,
          milestones,
          sources: parsed.sources || searchResults.map(r => r.url),
          reasoning: parsed.reasoning || 'AI extracted data points with full lifecycle coverage',
        };
      }
    } catch {
    }

    return this.generateLifecycleDefaults(searchResults);
  }

  private generateLifecycleDefaults(searchResults: SearchResult[]): AiSCurveDataResult {
    const now = new Date().getFullYear();
    return {
      dataPoints: [
        { x: now - 30, y: 10, source: 'estimated' },
        { x: now - 25, y: 20, source: 'estimated' },
        { x: now - 20, y: 40, source: 'estimated' },
        { x: now - 15, y: 80, source: 'estimated' },
        { x: now - 10, y: 150, source: 'estimated' },
        { x: now - 5, y: 220, source: 'estimated' },
        { x: now - 2, y: 280, source: 'estimated' },
        { x: now, y: 300, source: 'estimated' },
      ],
      milestones: [
        { year: now - 30, label: 'Initial Research', description: 'Fundamental research begins', type: 'invention' },
        { year: now - 20, label: 'First Prototype', description: 'Proof of concept demonstrated', type: 'breakthrough' },
        { year: now - 10, label: 'Commercial Launch', description: 'First commercial products released', type: 'commercialization' },
        { year: now - 5, label: 'Industry Standard', description: 'Standards established by industry bodies', type: 'standardization' },
        { year: now, label: 'Market Saturation', description: 'Growth slowing, approaching limits', type: 'peak' },
      ],
      sources: searchResults.map(r => r.url),
      reasoning: 'Default lifecycle-spanning data points (AI-estimated). Covers infancy → growth → maturity stages.',
    };
  }

  async close(): Promise<void> {
    if (this.agent) {
      await this.agent.close();
      this.agent = null;
    }
  }
}
