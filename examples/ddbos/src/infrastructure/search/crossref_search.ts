import { SearchResult, SearchQuery, SearchService } from '../../domain/solution/search_port.js';
import { ReferenceSourceType } from '../../domain/solution/external_reference.js';

export interface CrossRefConfig {
  baseUrl?: string;
  email?: string;
}

export class CrossRefSearchService implements SearchService {
  private config: CrossRefConfig;

  constructor(config: CrossRefConfig = {}) {
    this.config = config;
  }

  async search(_query: SearchQuery): Promise<SearchResult[]> {
    return [];
  }

  async searchPatents(_query: string, _maxResults = 5): Promise<SearchResult[]> {
    return [];
  }

  async searchPapers(query: string, maxResults = 5): Promise<SearchResult[]> {
    const baseUrl = this.config.baseUrl || 'https://api.crossref.org';
    const email = this.config.email ? `mailto=${this.config.email}` : '';

    const url = `${baseUrl}/works?query=${encodeURIComponent(query)}&select=title,abstract,author,published,URL,DOI&rows=${maxResults}${email ? `&${email}` : ''}`;

    const response = await fetch(url, {
      headers: {
        'User-Agent': 'TRIZ-Research-Tool/1.0 (https://github.com/triz-tool)',
      },
    });

    if (!response.ok) return [];

    const data: any = await response.json();
    const items = data.message?.items || [];

    return items.map((item: any) => {
      const pubDate = item.published?.['date-parts']?.[0]?.[0]
        || item['published-print']?.['date-parts']?.[0]?.[0]
        || item['published-online']?.['date-parts']?.[0]?.[0]
        || item.created?.['date-parts']?.[0]?.[0];

      return {
        title: item.title?.[0] || 'Unknown Paper',
        url: item.URL || `https://doi.org/${item.DOI}`,
        snippet: item.abstract || '',
        sourceType: 'paper' as ReferenceSourceType,
        publishedDate: pubDate ? String(pubDate) : undefined,
        authors: item.author?.map((a: any) => {
        const name = [a.given, a.family].filter(Boolean).join(' ').trim();
        return name || a.name || 'Unknown';
      }).filter(n => n !== 'Unknown') || undefined,
      };
    });
  }

  async searchTechSolutions(_query: string, _maxResults = 5): Promise<SearchResult[]> {
    return [];
  }
}
