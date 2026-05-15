import { SearchResult, SearchQuery, SearchService } from '../../domain/solution/search_port.js';
import { ReferenceSourceType } from '../../domain/solution/external_reference.js';

export interface OpenAlexConfig {
  baseUrl?: string;
}

export class OpenAlexSearchService implements SearchService {
  private config: OpenAlexConfig;

  constructor(config: OpenAlexConfig = {}) {
    this.config = config;
  }

  async search(_query: SearchQuery): Promise<SearchResult[]> {
    return [];
  }

  async searchPatents(query: string, maxResults = 5): Promise<SearchResult[]> {
    const baseUrl = this.config.baseUrl || 'https://api.openalex.org';

    const response = await fetch(
      `${baseUrl}/works?search=${encodeURIComponent(query + ' patent')}&per_page=${maxResults}`,
    );

    if (!response.ok) return [];

    const data: any = await response.json();
    const works = data.results || [];

    return works.map((w: any) => ({
      title: w.title || 'Unknown Patent',
      url: w.primary_location?.source?.url || w.doi || '',
      snippet: w.abstract || '',
      sourceType: 'patent' as ReferenceSourceType,
      publishedDate: w.publication_date || undefined,
      authors: w.authorships?.map((a: any) => a.author?.display_name).filter(Boolean) || undefined,
    }));
  }

  async searchPapers(query: string, maxResults = 5): Promise<SearchResult[]> {
    const baseUrl = this.config.baseUrl || 'https://api.openalex.org';

    const response = await fetch(
      `${baseUrl}/works?search=${encodeURIComponent(query)}&per_page=${maxResults}`,
    );

    if (!response.ok) return [];

    const data: any = await response.json();
    const works = data.results || [];

    return works.map((w: any) => ({
      title: w.title || 'Unknown Paper',
      url: w.primary_location?.source?.url || w.doi || '',
      snippet: w.abstract || '',
      sourceType: 'paper' as ReferenceSourceType,
      publishedDate: w.publication_date || undefined,
      authors: w.authorships?.map((a: any) => a.author?.display_name).filter(Boolean) || undefined,
    }));
  }

  async searchTechSolutions(query: string, maxResults = 5): Promise<SearchResult[]> {
    const baseUrl = this.config.baseUrl || 'https://api.openalex.org';

    const response = await fetch(
      `${baseUrl}/works?search=${encodeURIComponent(query + ' engineering solution')}&per_page=${maxResults}`,
    );

    if (!response.ok) return [];

    const data: any = await response.json();
    const works = data.results || [];

    return works.map((w: any) => ({
      title: w.title || '',
      url: w.primary_location?.source?.url || w.doi || '',
      snippet: w.abstract || '',
      sourceType: 'tech_solution' as ReferenceSourceType,
      publishedDate: w.publication_date || undefined,
      authors: w.authorships?.map((a: any) => a.author?.display_name).filter(Boolean) || undefined,
    }));
  }
}
