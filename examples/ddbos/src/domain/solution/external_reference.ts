export type ReferenceSourceType = 'patent' | 'paper' | 'tech_solution' | 'news' | 'blog';

export interface ExternalReference {
  url: string;
  title: string;
  sourceType: ReferenceSourceType;
  snippet: string;
  summary?: string;
  relevanceScore: number;
  publishedDate?: string;
  authors?: string[];
  fetchFullContent?: boolean;
}

export function createReference(
  url: string,
  title: string,
  sourceType: ReferenceSourceType,
  snippet: string,
  relevanceScore: number,
  publishedDate?: string,
  authors?: string[],
): ExternalReference {
  return {
    url,
    title,
    sourceType,
    snippet,
    relevanceScore,
    publishedDate,
    authors,
  };
}

export function enrichReferenceWithSummary(
  ref: ExternalReference,
  summary: string,
): ExternalReference {
  return { ...ref, summary };
}
