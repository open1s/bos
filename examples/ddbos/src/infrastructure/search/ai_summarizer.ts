import { Agent, BrainOS } from '@open1s/ezbos';
import { streamAgent } from '../ai/streaming.js';
import { LocaleConfig, DEFAULT_LOCALE, getLanguagePrompt } from '../../domain/shared/i18n.js';

export interface SummarizationResult {
  summary: string;
  keyFindings: string[];
  relevanceToProblem: string;
  trizPrinciples: string[];
  confidence?: number;
}

export interface SummarizeOptions {
  onThinking?: (text: string) => void;
  showThinking?: boolean;
}

export class AISummarizer {
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

    const langPrefix = this.locale.language === 'zh'
      ? '【中文模式】你必须用中文进行所有思考、推理和输出。\n\n'
      : '';

    const builder = this.brain.agent('triz-summarizer')
      .with_systemPrompt(`${langPrefix}You are a technical research summarizer specializing in TRIZ and engineering solutions.

For each document (patent, paper, or technical article), provide:
1. A concise summary (2-3 sentences)
2. Key findings as bullet points
3. How it relates to the user's problem
4. Which TRIZ inventive principles it demonstrates (if any)

Be precise, technical, and actionable.`)
      .with_temperature(0.3);

    this.agent = await builder.start();
  }

  async summarizeSnippet(
    title: string,
    snippet: string,
    problemDescription: string,
    options?: SummarizeOptions,
  ): Promise<SummarizationResult> {
    if (!this.agent) await this.initialize();

    const hasContent = snippet && snippet.trim().length > 10;
    const showThinking = options?.showThinking ?? false;

    const prompt = hasContent
      ? `Summarize this search result in the context of the given problem.

Problem: ${problemDescription}

Title: ${title}
Snippet: ${snippet}

Provide:
1. Summary (2-3 sentences)
2. Key findings (bullet points)
3. Relevance to the problem
4. TRIZ principles demonstrated (if any)

${getLanguagePrompt(this.locale.language)}`
      : `This search result has no snippet content. Based ONLY on the title, provide a brief assessment.

Problem: ${problemDescription}

Title: ${title}

Provide:
1. Summary: What this title suggests about the topic
2. Key findings: Likely focus areas based on title alone
3. Relevance: How this MIGHT relate to the problem (acknowledge uncertainty)
4. TRIZ Principles: Possible principles if the title suggests a known approach

Keep it brief. Acknowledge that no snippet content is available.

${getLanguagePrompt(this.locale.language)}`;

    const response = await streamAgent(this.agent!, prompt, {
      onThinking: options?.onThinking,
      showThinking,
    } as any);
    return this.parseResponse(response);
  }

  async summarizeFullContent(
    title: string,
    content: string,
    problemDescription: string,
    options?: SummarizeOptions,
  ): Promise<SummarizationResult> {
    if (!this.agent) await this.initialize();

    const truncated = content.slice(0, 8000);

    const prompt = `Summarize this full document in the context of the given problem.

Problem: ${problemDescription}

Title: ${title}
Content: ${truncated}

Provide:
1. Summary (2-3 sentences)
2. Key findings (bullet points)
3. Relevance to the problem
4. TRIZ principles demonstrated (if any)

${getLanguagePrompt(this.locale.language)}`;

    const response = await streamAgent(this.agent!, prompt, {
      onThinking: options?.onThinking,
    } as any);
    return this.parseResponse(response);
  }

  async assessRelevance(
    snippet: string,
    problemDescription: string,
  ): Promise<number> {
    if (!this.agent) await this.initialize();

    const prompt = `Rate the relevance of this content to the problem on a scale of 0-100.

Problem: ${problemDescription}
Content: ${snippet}

Return ONLY a number between 0 and 100.`;

    const response = await streamAgent(this.agent!, prompt);
    const match = response.match(/\d+/);
    return match ? Math.min(100, parseInt(match[0])) : 50;
  }

  private parseResponse(response: string): SummarizationResult {
    const lines = response.split('\n').filter(l => l.trim());

    let summary = '';
    const keyFindings: string[] = [];
    let relevanceToProblem = '';
    const trizPrinciples: string[] = [];

    let section = 'summary';
    for (const line of lines) {
      const lower = line.toLowerCase();
      if (lower.includes('summary') || lower.includes('overview')) {
        section = 'summary';
        summary += line.replace(/^.*?:\s*/, '') + ' ';
      } else if (lower.includes('key finding') || lower.includes('finding') || lower.includes('bullet')) {
        section = 'findings';
      } else if (lower.includes('relevance') || lower.includes('relates')) {
        section = 'relevance';
        relevanceToProblem += line.replace(/^.*?:\s*/, '') + ' ';
      } else if (lower.includes('triz') || lower.includes('principle')) {
        section = 'principles';
        trizPrinciples.push(line.replace(/^.*?:\s*/, '').replace(/^[-*•]\s*/, ''));
      } else if (line.startsWith('-') || line.startsWith('*') || line.startsWith('•')) {
        if (section === 'findings') {
          keyFindings.push(line.replace(/^[-*•]\s*/, ''));
        } else if (section === 'principles') {
          trizPrinciples.push(line.replace(/^[-*•]\s*/, ''));
        } else {
          summary += line + ' ';
        }
      } else {
        if (section === 'summary') summary += line + ' ';
        else if (section === 'relevance') relevanceToProblem += line + ' ';
      }
    }

    return {
      summary: summary.trim() || response.slice(0, 200),
      keyFindings: keyFindings.length > 0 ? keyFindings : [response.slice(0, 100)],
      relevanceToProblem: relevanceToProblem.trim() || 'Not specified',
      trizPrinciples,
    };
  }

  async close(): Promise<void> {
    if (this.agent) {
      await this.agent.close();
      this.agent = null;
    }
  }
}
