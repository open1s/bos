import { Agent, AgentBuilder, BrainOS } from '@open1s/ezbos';
import { streamAgent } from './streaming.js';
import { InventivePrinciple } from '../../domain/principle/entity.js';
import { LocaleConfig, DEFAULT_LOCALE, getLanguagePrompt } from '../../domain/shared/i18n.js';

const TRIZ_SYSTEM_PROMPT = `You are a TRIZ (Theory of Inventive Problem Solving) expert AI agent.

Your role is to:
1. Analyze technical contradictions and suggest inventive solutions
2. Apply the 40 Inventive Principles to real-world problems
3. Provide insights on how to resolve contradictions creatively
4. Suggest Su-Field analysis improvements
5. Evaluate ideality of proposed solutions

TRIZ Framework:
- 40 Inventive Principles for solving technical contradictions
- Contradiction Matrix mapping improving/worsening parameters to principles
- Su-Field (Substance-Field) analysis for system modeling
- Ideality concept: Ideality = Benefits / (Costs + Harms)
- Trends of Technical System Evolution

When analyzing a problem:
1. Identify the core contradiction (what improves vs what worsens)
2. Map to TRIZ parameters (1-39)
3. Look up recommended principles from the contradiction matrix
4. Provide concrete, actionable solution ideas based on those principles
5. Consider how multiple principles can be combined

Always provide practical, specific examples. Ground your suggestions in the actual problem context.`;

export class AiTrizAgent {
  private agent: Agent | null = null;
  private brain: BrainOS | null = null;
  private agentName: string;
  private locale: LocaleConfig;

  constructor(brainOrName: BrainOS | string, agentName = 'triz-expert', locale?: LocaleConfig) {
    this.locale = locale || DEFAULT_LOCALE;
    if (typeof brainOrName === 'string') {
      this.agentName = brainOrName;
    } else {
      this.brain = brainOrName;
      this.agentName = agentName;
    }
  }

  async initialize(): Promise<void> {
    if (!this.brain) {
      this.brain = new BrainOS();
      await this.brain.start();
    }

    const langPrefix = this.locale.language === 'zh'
      ? '【中文模式】你必须用中文进行所有思考、推理和输出。\n\n'
      : '';

    const builder = this.brain.agent(this.agentName)
      .with_systemPrompt(`${langPrefix}${TRIZ_SYSTEM_PROMPT}`)
      .with_temperature(0.7);

    this.agent = await builder.start();
  }

  async generateInsight(
    problemDescription: string,
    principle: InventivePrinciple,
    context?: string,
  ): Promise<string> {
    if (!this.agent) await this.initialize();

    const prompt = `Given this problem: "${problemDescription}"
${context ? `Context: ${context}` : ''}

Apply TRIZ Inventive Principle #${principle.index}: "${principle.name}"
Description: ${principle.description}
Examples: ${principle.examples.join(', ')}

Provide a specific, actionable insight on how to apply this principle to solve the problem.
Include:
1. How the principle applies to this specific problem
2. Concrete implementation steps
3. Potential challenges and how to overcome them
4. Any related principles that could enhance this solution

${getLanguagePrompt(this.locale.language)}`;

    return streamAgent(this.agent!, prompt);
  }

  async analyzeContradiction(
    improvingParam: string,
    worseningParam: string,
    description: string,
  ): Promise<string> {
    if (!this.agent) await this.initialize();

    const prompt = `Analyze this technical contradiction:

Improving parameter: ${improvingParam}
Worsening parameter: ${worseningParam}
Problem description: ${description}

Provide:
1. Root cause analysis of why this contradiction exists
2. Suggested TRIZ principles to apply
3. Creative solution concepts
4. How to verify the solution works

${getLanguagePrompt(this.locale.language)}`;

    return streamAgent(this.agent!, prompt);
  }

  async evaluateSolution(
    solution: string,
    criteria: string[],
  ): Promise<string> {
    if (!this.agent) await this.initialize();

    const prompt = `Evaluate this TRIZ-based solution:

Solution: ${solution}

Evaluation criteria: ${criteria.join(', ')}

Provide:
1. Strengths of the solution
2. Weaknesses or risks
3. Feasibility assessment
4. Suggestions for improvement
5. Overall recommendation (proceed/modify/reject)

${getLanguagePrompt(this.locale.language)}`;

    return streamAgent(this.agent!, prompt);
  }

  async suggestSuFieldImprovement(
    substance1: string,
    substance2: string,
    field: string,
    problem: string,
  ): Promise<string> {
    if (!this.agent) await this.initialize();

    const prompt = `Analyze this Su-Field model:

Substance 1 (tool): ${substance1}
Substance 2 (object): ${substance2}
Field: ${field}
Problem: ${problem}

Suggest improvements using the 76 Standard Solutions:
1. Identify the Su-Field type (complete, incomplete, harmful, insufficient)
2. Recommend specific standard solutions
3. Provide implementation guidance

${getLanguagePrompt(this.locale.language)}`;

    return streamAgent(this.agent!, prompt);
  }

  async close(): Promise<void> {
    if (this.agent) {
      await this.agent.close();
      this.agent = null;
    }
  }
}
