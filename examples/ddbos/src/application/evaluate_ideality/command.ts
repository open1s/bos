export interface EvaluateIdealityCommand {
  problemId: string;
  benefits: string[];
  costs: string[];
  harms: string[];
}

export interface IdealityScore {
  score: number;
  level: 'low' | 'medium' | 'high' | 'ideal';
  breakdown: {
    benefits: number;
    costs: number;
    harms: number;
  };
  recommendations: string[];
}

export interface EvaluateIdealityResult {
  problemId: string;
  ideality: IdealityScore;
}
