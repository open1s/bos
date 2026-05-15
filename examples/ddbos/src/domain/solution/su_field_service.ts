import { SuFieldComponents } from '../problem/value_objects.js';

export type SuFieldType = 'complete' | 'incomplete' | 'harmful' | 'insufficient' | 'excessive';

export interface SuFieldAnalysisResult {
  type: SuFieldType;
  diagnosis: string;
  standardSolutions: string[];
  recommendedAction: string;
}

export class SuFieldAnalysisService {
  analyze(components: SuFieldComponents): SuFieldAnalysisResult {
    const hasS1 = components.substance1.trim().length > 0;
    const hasS2 = components.substance2.trim().length > 0;
    const hasField = components.field.trim().length > 0;

    if (!hasS1 || !hasS2 || !hasField) {
      return {
        type: 'incomplete',
        diagnosis: 'Su-Field model is incomplete. Missing components detected.',
        standardSolutions: [
          'Complete the Su-Field by adding the missing substance or field',
          'Introduce a temporary substance to complete the interaction',
          'Use a field that already exists in the system environment',
        ],
        recommendedAction: 'Identify and add the missing component to complete the Su-Field triangle.',
      };
    }

    return {
      type: 'complete',
      diagnosis: `Complete Su-Field: ${components.substance1} acts on ${components.substance2} via ${components.field}.`,
      standardSolutions: this.getStandardSolutions(components),
      recommendedAction: 'Evaluate if the interaction is harmful, insufficient, or excessive.',
    };
  }

  analyzeHarmful(s1: string, s2: string, field: string): SuFieldAnalysisResult {
    return {
      type: 'harmful',
      diagnosis: `Harmful Su-Field: ${s1} produces harmful effects on ${s2} via ${field}.`,
      standardSolutions: [
        'Introduce a third substance S3 between S1 and S2 to block harmful interaction',
        'Modify S1 or S2 to eliminate the harmful effect',
        'Replace the field with a different type that does not cause harm',
        'Introduce a counter-field that neutralizes the harmful effect',
      ],
      recommendedAction: 'Apply standard solutions 1.1.x or 1.2.x to eliminate harmful interaction.',
    };
  }

  analyzeInsufficient(s1: string, s2: string, field: string): SuFieldAnalysisResult {
    return {
      type: 'insufficient',
      diagnosis: `Insufficient Su-Field: ${s1} does not adequately affect ${s2} via ${field}.`,
      standardSolutions: [
        'Increase the intensity of the existing field',
        'Add a second field to enhance the interaction',
        'Replace the field with a more effective type',
        'Modify S1 to be more responsive to the field',
      ],
      recommendedAction: 'Apply standard solutions 2.1.x to enhance the insufficient interaction.',
    };
  }

  private getStandardSolutions(components: SuFieldComponents): string[] {
    return [
      `Review interaction between ${components.substance1} and ${components.substance2}`,
      'Check if the Su-Field model matches the 76 standard solution patterns',
      'Consider if a phase transition could improve the interaction',
      'Evaluate if introducing a new field would enhance the system',
    ];
  }
}
