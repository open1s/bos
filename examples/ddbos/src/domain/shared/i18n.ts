export type Language = 'zh' | 'en';

export interface LocaleConfig {
  language: Language;
}

export const DEFAULT_LOCALE: LocaleConfig = { language: 'zh' };

export const STAGE_LABELS: Readonly<Record<string, Record<Language, string>>> = {
  infancy: { zh: '萌芽期', en: 'Infancy' },
  growth: { zh: '成长期', en: 'Growth' },
  maturity: { zh: '成熟期', en: 'Maturity' },
  decline: { zh: '衰退期', en: 'Decline' },
};

export const STAGE_DESCRIPTIONS: Readonly<Record<string, Record<Language, string>>> = {
  infancy: {
    zh: '早期研发阶段。进展缓慢，投入高，许多死胡同。专注于基础研究。',
    en: 'Early R&D phase. Slow progress, high investment, many dead ends. Focus on fundamental research.',
  },
  growth: {
    zh: '快速改进阶段。突破加速发展。大量投资获得回报。市场接受度提高。',
    en: 'Rapid improvement phase. Breakthroughs accelerate. Heavy investment pays off. Market adoption increases.',
  },
  maturity: {
    zh: '收益递减。大部分简单问题已解决。仅 incremental 改进。专注于降低成本。',
    en: 'Diminishing returns. Most easy problems solved. Incremental improvements only. Focus on cost reduction.',
  },
  decline: {
    zh: '技术正在被替代。新的S曲线出现。撤资并过渡到下一代技术。',
    en: 'Technology being replaced. New S-curve emerging. Divest and transition to next-generation technology.',
  },
};

export const STAGE_STRATEGIES: Readonly<Record<string, Record<Language, string>>> = {
  infancy: {
    zh: '投资基础研究。保护知识产权。探索多种方案。接受高失败率。',
    en: 'Invest in fundamental research. Protect IP. Explore multiple approaches. Accept high failure rate.',
  },
  growth: {
    zh: '加速发展。扩大生产。建立市场地位。积极申请专利。',
    en: 'Accelerate development. Scale production. Build market position. Patent aggressively.',
  },
  maturity: {
    zh: '优化成本和可靠性。最大化价值。开始投资下一代技术。',
    en: 'Optimize for cost and reliability. Extract maximum value. Begin investing in next-generation technology.',
  },
  decline: {
    zh: '减少投资。将客户迁移到S2技术。收获剩余利润。剥离资产。',
    en: 'Phase out investment. Migrate customers to S2 technology. Harvest remaining profits. Divest assets.',
  },
};

export const TRL_TITLES: Readonly<Record<number, Record<Language, string>>> = {
  1: { zh: '观察到基本原理', en: 'Basic Principles Observed' },
  2: { zh: '技术概念形成', en: 'Technology Concept Formulated' },
  3: { zh: '实验概念验证', en: 'Experimental Proof of Concept' },
  4: { zh: '实验室组件验证', en: 'Component Validation in Laboratory' },
  5: { zh: '相关环境组件验证', en: 'Component Validation in Relevant Environment' },
  6: { zh: '相关环境系统/子系统模型', en: 'System/Subsystem Model in Relevant Environment' },
  7: { zh: '运行环境系统原型', en: 'System Prototype in Operational Environment' },
  8: { zh: '实际系统完成并合格', en: 'Actual System Completed and Qualified' },
  9: { zh: '实际系统在常规使用中验证', en: 'Actual System Proven in Operational Environment' },
};

export const TRL_DESCRIPTIONS: Readonly<Record<number, Record<Language, string>>> = {
  1: {
    zh: '观察和报告基本原理。科学研究开始转化为应用研究。',
    en: 'Basic principles observed and reported. Scientific research begins to be translated into applied research.',
  },
  2: {
    zh: '技术概念和/或应用已形成。发明开始，但无可用证据。',
    en: 'Technology concept and/or application formulated. Invention begins, but no proof available.',
  },
  3: {
    zh: '启动积极研发。获得分析和实验概念验证。',
    en: 'Active R&D initiated. Analytical and experimental proof of concept obtained.',
  },
  4: {
    zh: '实验室环境中的组件和/或面包板验证。',
    en: 'Component and/or breadboard validation in laboratory environment.',
  },
  5: {
    zh: '相关环境中的组件验证。显著提高保真度。',
    en: 'Component validation in relevant environment. Significantly increases fidelity.',
  },
  6: {
    zh: '相关环境中系统/子系统模型或原型演示。',
    en: 'System/subsystem model or prototype demonstration in relevant environment.',
  },
  7: {
    zh: '运行环境中系统原型演示。',
    en: 'System prototype demonstration in operational environment.',
  },
  8: {
    zh: '实际系统已完成并通过测试和演示验证。',
    en: 'Actual system completed and qualified through test and demonstration.',
  },
  9: {
    zh: '实际系统通过常规使用中的成功任务操作验证。',
    en: 'Actual system proven through successful mission operations in routine use.',
  },
};

export const MILESTONE_LABELS: Readonly<Record<string, Record<Language, string>>> = {
  invention: { zh: '发明', en: 'Invention' },
  breakthrough: { zh: '突破', en: 'Breakthrough' },
  commercialization: { zh: '商业化', en: 'Commercialization' },
  standardization: { zh: '标准化', en: 'Standardization' },
  peak: { zh: '巅峰', en: 'Peak' },
  decline: { zh: '衰退', en: 'Decline' },
};

export const SVG_LABELS: Readonly<Record<string, Record<Language, string>>> = {
  inflectionPoint: { zh: '拐点', en: 'Inflection Point' },
  crossover: { zh: 'S1/S2 交叉点', en: 'S1/S2 Crossover' },
  s1Peak: { zh: 'S1 峰值', en: 'S1 Peak' },
  timeAxis: { zh: '时间（年）', en: 'Time (Year)' },
  s1Current: { zh: 'S1: 当前技术', en: 'S1: Current Technology' },
  s2Next: { zh: 'S2: 下一代', en: 'S2: Next Generation' },
  realData: { zh: '真实数据点', en: 'Real Data Points' },
  analysisSummary: { zh: '分析摘要', en: 'Analysis Summary' },
  strategy: { zh: '策略', en: 'Strategy' },
  keyEvents: { zh: '关键事件', en: 'Key Events' },
  scurveAnalysis: { zh: 'S曲线分析', en: 'S-Curve Analysis' },
  currentStage: { zh: '当前阶段', en: 'Current Stage' },
  s2Stage: { zh: 'S2 阶段', en: 'S2 Stage' },
  crossoverYear: { zh: '交叉年份', en: 'Crossover Year' },
  maxS1: { zh: 'S1 最大值', en: 'Max S1' },
  maxS2: { zh: 'S2 最大值', en: 'Max S2' },
};

export const REPORT_LABELS: Readonly<Record<string, Record<Language, string>>> = {
  title: { zh: 'TRIZ 研究报告', en: 'TRIZ Research Report' },
  problem: { zh: '问题', en: 'Problem' },
  date: { zh: '日期', en: 'Date' },
  executiveSummary: { zh: '执行摘要', en: 'Executive Summary' },
  priorArtAnalysis: { zh: '现有技术分析', en: 'Prior Art Analysis' },
  patentLandscape: { zh: '专利格局', en: 'Patent Landscape' },
  academicResearch: { zh: '学术研究', en: 'Academic Research' },
  techSolutions: { zh: '技术方案', en: 'Technical Solutions' },
  keyInsight: { zh: '关键洞察', en: 'Key Insight' },
  contradictionAnalysis: { zh: '矛盾分析', en: 'Contradiction Analysis' },
  improvingParameter: { zh: '改善参数', en: 'Improving Parameter' },
  worseningParameter: { zh: '恶化参数', en: 'Worsening Parameter' },
  recommendedPrinciples: { zh: '推荐原理', en: 'Recommended Principles' },
  technologyMaturity: { zh: '技术成熟度评估', en: 'Technology Maturity Assessment' },
  summary: { zh: '摘要', en: 'Summary' },
  currentTechnology: { zh: '当前技术 (S1)', en: 'Current Technology (S1)' },
  nextGenTechnology: { zh: '下一代技术 (S2)', en: 'Next-Generation Technology (S2)' },
  strategicWarning: { zh: '战略警告', en: 'Strategic Warning' },
  criticalAlert: { zh: '紧急警告', en: 'Critical Alert' },
  recommendations: { zh: '建议', en: 'Recommendations' },
  immediateActions: { zh: '立即行动', en: 'Immediate Actions' },
  researchPriorities: { zh: '研究优先级', en: 'Research Priorities' },
  analysisErrors: { zh: '分析错误与警告', en: 'Analysis Errors & Warnings' },
  metadata: { zh: '研究元数据', en: 'Research Metadata' },
  duration: { zh: '持续时间', en: 'Duration' },
  sourcesUsed: { zh: '使用数据源', en: 'Sources Used' },
  aiCalls: { zh: 'AI 调用次数', en: 'AI Calls Made' },
  errors: { zh: '错误数', en: 'Errors' },
  sCurveStage: { zh: 'S曲线阶段', en: 'S-Curve Stage' },
  trl: { zh: '技术就绪指数', en: 'TRL' },
  confidence: { zh: '置信度', en: 'Confidence' },
  dataPoints: { zh: '数据点', en: 'Data Points' },
  estimated: { zh: '估计', en: 'estimated' },
  real: { zh: '真实', en: 'real' },
  metric: { zh: '指标', en: 'Metric' },
  value: { zh: '值', en: 'Value' },
  sCurveCrossover: { zh: 'S曲线交叉点', en: 'S-Curve Crossover' },
  scurveVisualization: { zh: 'S曲线可视化', en: 'S-Curve Visualization' },
  keyEventsMilestones: { zh: '关键事件与里程碑', en: 'Key Events & Milestones' },
  year: { zh: '年份', en: 'Year' },
  event: { zh: '事件', en: 'Event' },
  type: { zh: '类型', en: 'Type' },
  aiEstimate: { zh: 'AI 估计', en: 'AI estimate' },
  aiEstimatedData: { zh: 'AI 估计数据', en: 'AI-estimated data' },
  strategy: { zh: '策略', en: 'Strategy' },
  patentLandscapeSummary: { zh: '专利格局揭示了', en: 'The patent landscape reveals' },
  keyPlayers: { zh: '主要参与者包括', en: 'Key players include' },
  patentsSpan: { zh: '专利时间跨度从', en: 'The patents span from' },
  indicating: { zh: '表明', en: 'indicating' },
  academicSummary: { zh: '学术研究提供了', en: 'Academic research provides' },
  recentWork: { zh: '最近的工作由', en: 'Recent work by' },
  demonstrates: { zh: '展示了', en: 'demonstrates' },
  techSolutionSummary: { zh: '实际实现展示了', en: 'practical implementations demonstrate' },
  show: { zh: '这些方案显示', en: 'These solutions show' },
  maturityHigh: { zh: '高度成熟的技术，已证明商业可行性。重点应转向优化和降低成本。', en: 'a highly mature technology with proven commercial viability. Focus should shift to optimization and cost reduction.' },
  maturityMid: { zh: '接近商业就绪的技术。加速验证并准备进入市场。', en: 'a technology approaching commercial readiness. Accelerate validation and prepare for market entry.' },
  maturityLow: { zh: '积极开发中的技术，前景良好。继续原型设计并寻求合作。', en: 'a technology in active development with promising prospects. Continue prototyping and seek partnerships.' },
  maturityEarly: { zh: '高潜力但开发风险大的新兴技术。投资基础研究。', en: 'an emerging technology with high potential but significant development risk. Invest in fundamental research.' },
  strategyInvest: { zh: '投资基础研究。探索多种方法。保护知识产权。接受高失败率。', en: 'Invest in fundamental research. Explore multiple approaches. Protect IP. Accept high failure rate.' },
  strategyAccelerate: { zh: '加速发展。扩大生产。建立市场地位。积极申请专利。', en: 'Accelerate development. Scale production. Build market position. Patent aggressively.' },
  strategyOptimize: { zh: '优化成本和可靠性。提取最大价值。开始投资下一代技术。', en: 'Optimize for cost and reliability. Extract maximum value. Begin investing in next-generation technology.' },
  strategyPhaseOut: { zh: '逐步减少投资。将客户迁移到下一代技术。收获剩余利润。', en: 'Phase out investment. Migrate customers to next-generation technology. Harvest remaining profits.' },
  years: { zh: '年', en: 'years' },
  willSurpass: { zh: '下一代技术将在', en: 'The next-generation technology will surpass current performance in' },
  beginInvesting: { zh: '现在开始投资S2技术以保持竞争优势。', en: 'Begin investing in S2 technology now to maintain competitive advantage.' },
  hasSurpassed: { zh: '下一代技术已超过当前性能。', en: 'The next-generation technology has already surpassed current performance.' },
  immediateTransition: { zh: '需要立即过渡到S2。', en: 'Immediate transition to S2 is required.' },
  svgChart: { zh: 'SVG 图表', en: 'SVG Chart' },
  asciiPreview: { zh: 'ASCII 预览', en: 'ASCII Preview' },
  applyPrinciple: { zh: '应用TRIZ原理', en: 'Apply TRIZ Principle' },
  reviewPatents: { zh: '审查已识别的', en: 'Review the' },
  identifiedPatents: { zh: '项已识别专利以绘制竞争格局并识别空白机会。', en: 'identified patents to map the competitive landscape and identify white space opportunities.' },
  studyPapers: { zh: '研究', en: 'Study the' },
  academicPapers: { zh: '篇学术论文以理解潜在解决方案的理论基础。', en: 'academic papers to understand the theoretical basis for potential solutions.' },
  techRoadmap: { zh: '基于S曲线分析，制定技术过渡计划，平衡S1优化与S2投资。', en: 'Based on the S-curve analysis, develop a technology transition plan that balances S1 optimization with S2 investment.' },
  searchKeywords: { zh: '搜索关键词', en: 'Search Keywords' },
  patents: { zh: '专利', en: 'Patents' },
  academicPapersTitle: { zh: '学术论文', en: 'Academic Papers' },
  technicalSolutions: { zh: '技术方案', en: 'Technical Solutions' },
  trizContradictionAnalysis: { zh: 'TRIZ 矛盾分析', en: 'TRIZ Contradiction Analysis' },
  improving: { zh: '改善', en: 'Improving' },
  worsening: { zh: '恶化', en: 'Worsening' },
  nextGenTRL: { zh: '下一代TRL', en: 'Next-Gen TRL' },
  crossover: { zh: '交叉点', en: 'Crossover' },
  scurveChart: { zh: 'S曲线图表', en: 'S-Curve Chart' },
  scurvePreview: { zh: 'S曲线预览', en: 'S-Curve Preview' },
  noRecommendations: { zh: '暂无具体建议。请审查现有技术以获取洞察。', en: 'No specific recommendations available. Review prior art for insights.' },
  urgent: { zh: '紧急', en: 'URGENT' },
  opportunity: { zh: '机会', en: 'OPPORTUNITY' },
  earlyStage: { zh: '早期阶段', en: 'EARLY STAGE' },
  surpassByYear: { zh: '下一代S曲线将在', en: 'The next S-curve will surpass current performance by year' },
  beginTransitioning: { zh: '立即开始将资源过渡到S2技术。', en: 'Begin transitioning resources to S2 technology immediately.' },
  continueInvesting: { zh: '继续投资S1，同时开始S2的探索性研发。', en: 'Continue investing in S1 while starting exploratory R&D for S2.' },
  focusResearch: { zh: '专注于基础研究和知识产权保护。', en: 'Focus on fundamental research and IP protection.' },
  s1CurrentTech: { zh: 'S1: 当前技术', en: 'S1: Current Technology' },
  s2NextGenTech: { zh: 'S2: 下一代技术', en: 'S2: Next Generation Technology' },
  strategicInsight: { zh: '战略洞察', en: 'Strategic Insight' },
  trlReconciliation: { zh: 'TRL 协调', en: 'TRL Reconciliation' },
  inYears: { zh: '年后', en: 'in' },
  yearsAgo: { zh: '年前', en: 'years ago' },
  performanceGap: { zh: '性能差距', en: 'Performance Gap' },
  higherThanS1: { zh: '高于S1', en: 'higher than S1' },
  predictedIn: { zh: 'S2交叉点预计在', en: 'S2 crossover predicted in' },
  exceedAround: { zh: 'S2性能将在', en: 'S2 performance will exceed S1 around year' },
  reducingCosts: { zh: '减少成本和资源消耗', en: 'Reduce costs and resource consumption' },
  eliminatingHarms: { zh: '消除有害功能和副作用', en: 'Eliminate harms and negative side effects' },
  increasingBenefits: { zh: '增加有用功能和收益', en: 'Increase useful functions and benefits' },
  considerPrinciples: { zh: '考虑应用发明原理来进一步改进', en: 'Consider applying inventive principles to further improve' },
  patentTrendGrowing: { zh: '该领域专利活动不断增长', en: 'growing patent activity in this domain' },
  patentTrendMature: { zh: '该领域专利格局成熟稳定', en: 'a mature and stable patent landscape' },
  patentTrendEmerging: { zh: '新兴技术领域', en: 'an emerging technology space' },
  patentInsight: { zh: '这些专利展示了处理类似技术挑战的多种方法。分析权利要求以了解保护范围并识别设计自由空间。', en: 'These patents demonstrate multiple approaches to addressing similar technical challenges. Analyze the claims to understand protection scope and identify design-around opportunities.' },
  researchTrend: { zh: '该领域的学术研究不断增长，实际应用正在涌现', en: 'growing academic interest in this domain with practical applications emerging' },
  researchInsight: { zh: '这些论文为潜在解决方案提供了理论基础和实验验证。重点关注高被引论文以获取最有影响力的研究。', en: 'These papers provide theoretical foundations and experimental validation for potential solutions. Focus on papers with high citation counts for the most impactful research.' },
  techSolutionTrend: { zh: '实际实现变得越来越复杂和商业可行', en: 'practical implementations are becoming more sophisticated and commercially viable' },
  techSolutionInsight: { zh: '这些解决方案展示了TRIZ原理在现实应用中的实际实现。研究它们以获取可适应特定上下文的成熟方法。', en: 'These solutions demonstrate practical implementations of TRIZ principles in real-world applications. Study them for proven approaches that can be adapted to your specific context.' },
  applyTrizPrinciples: { zh: '应用TRIZ原理', en: 'Apply TRIZ principles' },
  reviewRelevantPatents: { zh: '审查', en: 'Review' },
  relevantPatents: { zh: '项相关专利以了解竞争格局', en: 'relevant patents to understand the competitive landscape' },
  studyRelevantPapers: { zh: '研究', en: 'Study' },
  relevantPapers: { zh: '篇相关论文以理解理论基础', en: 'relevant papers to understand the theoretical basis' },
  techMature: { zh: '技术已成熟。专注于优化和成本降低。', en: 'Technology is mature. Focus on optimization and cost reduction.' },
  techDeveloping: { zh: '技术正在发展。加速开发和专利申请。', en: 'Technology is developing. Accelerate development and patent filing.' },
  techEarly: { zh: '技术处于早期阶段。投资基础研究。', en: 'Technology is in early stage. Invest in fundamental research.' },
  monitorS2: { zh: '监控S2曲线开发。目前处于', en: 'Monitor S2 curve development. Currently in' },
  stage: { zh: '阶段。开始下一代技术的探索性研发。', en: 'stage. Start exploratory R&D for next-generation technology.' },
  criticalS2: { zh: '关键：S2曲线（下一代', en: 'Critical: The S2 curve (' },
  nextGen: { zh: '）处于', en: 'next-gen) is in' },
  stageTransition: { zh: '阶段。立即开始将资源过渡到S2技术。', en: 'stage. Begin transitioning resources to S2 technology immediately.' },
  description: { zh: '描述', en: 'Description' },
  growthRate: { zh: '增长率', en: 'Growth Rate' },
  inflectionPoint: { zh: '拐点', en: 'Inflection Point' },
  mostLikely: { zh: '最可能', en: 'most likely' },
  userProvided: { zh: '用户提供', en: 'user-provided' },
  yourTech: { zh: '你的技术', en: 'Your technology' },
  isInStage: { zh: '处于', en: 'is in the' },
  found: { zh: '项', en: 'found' },
  authors: { zh: '作者', en: 'Authors' },
  aiEstimatedData: { zh: 'AI 估计数据', en: 'AI-estimated data' },
  source: { zh: '来源', en: 'Source' },
  reasoning: { zh: '推理', en: 'Reasoning' },
  analysisQuality: { zh: '分析质量', en: 'Analysis Quality' },
  warnings: { zh: '警告', en: 'warnings' },
  reviewErrors: { zh: '查看错误部分了解详情。', en: 'Review errors section for details.' },
  keyInsights: { zh: '关键洞察', en: 'Key Insights' },
  recommendedApproach: { zh: '推荐方法', en: 'Recommended Approach' },
  relevance: { zh: '相关性', en: 'Relevance' },
  keyFindings: { zh: '关键发现', en: 'Key Findings' },
  principles: { zh: '原理', en: 'Principles' },
  acceleratePrototyping: { zh: '加速原型设计和验证', en: 'Accelerate prototyping and validation' },
  investResearch: { zh: '投资基础研究', en: 'Invest in fundamental research' },
};

export function t(key: string, lang: Language): string {
  const map = REPORT_LABELS[key];
  if (!map) return key;
  return map[lang] || map.en || key;
}

export function stageLabel(stage: string, lang: Language): string {
  const map = STAGE_LABELS[stage];
  return map?.[lang] || stage;
}

export function stageDesc(stage: string, lang: Language): string {
  const map = STAGE_DESCRIPTIONS[stage];
  return map?.[lang] || map?.en || '';
}

export function stageStrategy(stage: string, lang: Language): string {
  const map = STAGE_STRATEGIES[stage];
  return map?.[lang] || map?.en || '';
}

export function trlTitle(level: number, lang: Language): string {
  const map = TRL_TITLES[level];
  return map?.[lang] || map?.en || '';
}

export function trlDesc(level: number, lang: Language): string {
  const map = TRL_DESCRIPTIONS[level];
  return map?.[lang] || map?.en || '';
}

export function milestoneLabel(type: string, lang: Language): string {
  const map = MILESTONE_LABELS[type];
  return map?.[lang] || type;
}

export function svgLabel(key: string, lang: Language): string {
  const map = SVG_LABELS[key];
  return map?.[lang] || map?.en || key;
}

export function getLanguagePrompt(lang: Language): string {
  return lang === 'zh'
    ? '请用中文回答。'
    : 'Please respond in English.';
}

export const PROGRESS_MESSAGES: Readonly<Record<string, Record<Language, string>>> = {
  extractingKeywords: { zh: 'AI 正在提取优化搜索关键词...', en: 'AI is extracting optimized search keywords...' },
  searching: { zh: '正在搜索专利、论文和技术方案...', en: 'Searching patents, papers, and technical solutions...' },
  foundResults: { zh: '找到', en: 'Found' },
  patents: { zh: '项专利', en: 'patents' },
  papers: { zh: '篇论文', en: 'papers' },
  techSolutions: { zh: '个技术方案', en: 'tech solutions' },
  analyzingSummarizing: { zh: 'AI 正在分析和总结每个结果...', en: 'AI is analyzing and summarizing each result...' },
  summarizationComplete: { zh: '总结完成', en: 'Summarization complete' },
  extractingTRIZ: { zh: 'AI 正在提取TRIZ参数并分析矛盾...', en: 'AI is extracting TRIZ parameters and analyzing contradictions...' },
  extracted: { zh: '已提取', en: 'Extracted' },
  runningTRIZ: { zh: '正在运行TRIZ矛盾矩阵查找、S曲线分析和TRL评估...', en: 'Running TRIZ contradiction matrix lookup, S-curve analysis, and TRL assessment...' },
  trizComplete: { zh: 'TRIZ分析完成', en: 'TRIZ analysis complete' },
  principles: { zh: '个原理', en: 'principles' },
  callingTool: { zh: '调用工具', en: 'Calling tool' },
  extractingSCurve: { zh: '正在提取S曲线数据', en: 'Extracting S-curve data' },
  dataPoints: { zh: '数据点', en: 'Data points' },
  fittingCurve: { zh: '正在拟合逻辑曲线并检测阶段...', en: 'Fitting logistic curve and detecting stage...' },
  stage: { zh: '阶段', en: 'stage' },
  crossover: { zh: '交叉点', en: 'crossover' },
  svgSaved: { zh: 'SVG已保存到', en: 'SVG saved to' },
  contradictionFailed: { zh: '矛盾分析失败', en: 'Contradiction analysis failed' },
  sCurveFailed: { zh: 'S曲线/TRL分析失败', en: 'S-curve/TRL analysis failed' },
  noRealData: { zh: '未找到真实S曲线数据。使用AI估计数据点。结果为近似值。', en: 'No real S-curve data found. Using AI-estimated data points. Results are approximate.' },
  failedInit: { zh: '初始化AI代理失败', en: 'Failed to initialize AI agent' },
  failedParse: { zh: '解析AI分析响应为JSON失败', en: 'Failed to parse AI analysis response as JSON' },
  failedSearch: { zh: '未找到现有技术。结果可能不太可靠。', en: 'No prior art found. Results may be less reliable.' },
  failedAnalyze: { zh: 'AI分析失败', en: 'AI analysis failed' },
  failedTRIZ: { zh: 'TRIZ分析失败', en: 'TRIZ analysis failed' },
};

export function progressMsg(key: string, lang: Language): string {
  const map = PROGRESS_MESSAGES[key];
  return map?.[lang] || map?.en || key;
}
