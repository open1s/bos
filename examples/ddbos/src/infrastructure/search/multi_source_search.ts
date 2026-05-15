import { SearchResult, SearchQuery, SearchService } from '../../domain/solution/search_port.js';
import { ReferenceSourceType } from '../../domain/solution/external_reference.js';
import { CrossRefSearchService } from './crossref_search.js';
import { OpenAlexSearchService } from './openalex_search.js';

export interface BraveSearchConfig {
  apiKey: string;
  baseUrl?: string;
}

export interface SerperConfig {
  apiKey: string;
  baseUrl?: string;
}

export interface SemanticScholarConfig {
  baseUrl?: string;
}

export interface MultiSourceSearchConfig {
  brave?: BraveSearchConfig;
  serper?: SerperConfig;
  semanticScholar?: SemanticScholarConfig;
  crossRef?: import('./crossref_search.js').CrossRefConfig;
  openAlex?: import('./openalex_search.js').OpenAlexConfig;
  defaultMaxResults?: number;
}

export class MultiSourceSearchService implements SearchService {
  private config: MultiSourceSearchConfig;
  private crossRefSearch: CrossRefSearchService;
  private openAlexSearch: OpenAlexSearchService;

  constructor(config: MultiSourceSearchConfig) {
    this.config = config;
    this.crossRefSearch = new CrossRefSearchService(config.crossRef);
    this.openAlexSearch = new OpenAlexSearchService(config.openAlex);
  }

  async search(query: SearchQuery): Promise<SearchResult[]> {
    const keywordStr = query.keywords.join(' ');
    const maxResults = query.maxResults || this.config.defaultMaxResults || 10;
    const results: SearchResult[] = [];

    if (query.sourceTypes.includes('patent') && this.config.serper) {
      const patents = await this.searchPatents(keywordStr, maxResults);
      results.push(...patents);
    }

    if (query.sourceTypes.includes('paper') && this.config.semanticScholar) {
      const papers = await this.searchPapers(keywordStr, maxResults);
      results.push(...papers);
    }

    if (query.sourceTypes.includes('tech_solution') && this.config.brave) {
      const tech = await this.searchTechSolutions(keywordStr, maxResults);
      results.push(...tech);
    }

    if (query.sourceTypes.includes('news') && this.config.brave) {
      const news = await this.searchNews(keywordStr, maxResults);
      results.push(...news);
    }

    if (query.sourceTypes.includes('blog') && this.config.brave) {
      const blogs = await this.searchBlogs(keywordStr, maxResults);
      results.push(...blogs);
    }

    results.sort((a, b) => (b.snippet.length + b.title.length) - (a.snippet.length + a.title.length));
    return results.slice(0, maxResults);
  }

  async searchPatents(query: string, maxResults = 5): Promise<SearchResult[]> {
    if (this.config.serper) {
      try {
        const response = await fetch(`${this.config.serper.baseUrl || 'https://google.serper.dev/patents'}`, {
          method: 'POST',
          headers: {
            'X-API-KEY': this.config.serper.apiKey,
            'Content-Type': 'application/json',
          },
          body: JSON.stringify({ q: query, num: maxResults }),
        });

        if (response.ok) {
          const data: any = await response.json();
          const patents = data.patents || data.organic || [];

          if (patents.length > 0) {
            return patents.slice(0, maxResults).map((p: any) => ({
              title: p.title || p.snippet?.split('\n')[0] || 'Unknown Patent',
              url: p.link || p.url || '',
              snippet: p.snippet || p.abstract || '',
              sourceType: 'patent' as ReferenceSourceType,
              publishedDate: p.date || p.publicationDate,
              authors: p.inventors ? [p.inventors].flat() : undefined,
            }));
          }
        }
      } catch {
      }
    }

    // Fallback to OpenAlex (free, no API key)
    return await this.openAlexSearch.searchPatents(query, maxResults);
  }

  async searchPapers(query: string, maxResults = 5): Promise<SearchResult[]> {
    const baseUrl = this.config.semanticScholar?.baseUrl || 'https://api.semanticscholar.org/graph/v1';

    try {
      const response = await fetch(
        `${baseUrl}/paper/search?query=${encodeURIComponent(query)}&limit=${maxResults}&fields=title,abstract,authors,year,externalIds,url`,
      );

      if (response.ok) {
        const data: any = await response.json();
        const papers = data.data || [];

        if (papers.length > 0) {
          return papers.map((p: any) => ({
            title: p.title || 'Unknown Paper',
            url: p.url || `https://www.semanticscholar.org/paper/${p.paperId}`,
            snippet: p.abstract || '',
            sourceType: 'paper' as ReferenceSourceType,
            publishedDate: p.year ? String(p.year) : undefined,
            authors: p.authors ? p.authors.map((a: any) => a.name) : undefined,
          }));
        }
      }

      // Semantic Scholar failed or rate-limited, try CrossRef
      return await this.crossRefSearch.searchPapers(query, maxResults);
    } catch {
      // Fallback to CrossRef on any error
      return await this.crossRefSearch.searchPapers(query, maxResults);
    }
  }

  async searchTechSolutions(query: string, maxResults = 5): Promise<SearchResult[]> {
    if (this.config.brave) {
      try {
        const response = await fetch(
          `https://api.search.brave.com/res/v1/web/search?q=${encodeURIComponent(query + ' technical solution engineering')}&count=${maxResults}`,
          {
            headers: {
              'Accept': 'application/json',
              'Accept-Encoding': 'gzip',
              'X-Subscription-Token': this.config.brave.apiKey,
            },
          },
        );

        if (response.ok) {
          const data: any = await response.json();
          const results = data.web?.results || [];

          if (results.length > 0) {
            return results.slice(0, maxResults).map((r: any) => ({
              title: r.title || '',
              url: r.url || '',
              snippet: r.description || '',
              sourceType: 'tech_solution' as ReferenceSourceType,
              publishedDate: r.page_age,
            }));
          }
        }
      } catch {
      }
    }

    // Fallback to OpenAlex (free, no API key)
    return await this.openAlexSearch.searchTechSolutions(query, maxResults);
  }

  async searchNews(query: string, maxResults = 5): Promise<SearchResult[]> {
    if (!this.config.brave) return [];

    const response = await fetch(
      `https://api.search.brave.com/res/v1/news/search?q=${encodeURIComponent(query)}&count=${maxResults}`,
      {
        headers: {
          'Accept': 'application/json',
          'Accept-Encoding': 'gzip',
          'X-Subscription-Token': this.config.brave.apiKey,
        },
      },
    );

    if (!response.ok) return [];

    const data: any = await response.json();
    const results = data.results || [];

    return results.slice(0, maxResults).map((r: any) => ({
      title: r.title || '',
      url: r.url || '',
      snippet: r.description || '',
      sourceType: 'news' as ReferenceSourceType,
      publishedDate: r.page_age,
    }));
  }

  async searchBlogs(query: string, maxResults = 5): Promise<SearchResult[]> {
    if (!this.config.brave) return [];

    const response = await fetch(
      `https://api.search.brave.com/res/v1/web/search?q=${encodeURIComponent(query + ' blog')}&count=${maxResults}`,
      {
        headers: {
          'Accept': 'application/json',
          'Accept-Encoding': 'gzip',
          'X-Subscription-Token': this.config.brave.apiKey,
        },
      },
    );

    if (!response.ok) return [];

    const data: any = await response.json();
    const results = data.web?.results || [];

    return results.slice(0, maxResults).map((r: any) => ({
      title: r.title || '',
      url: r.url || '',
      snippet: r.description || '',
      sourceType: 'blog' as ReferenceSourceType,
      publishedDate: r.page_age,
    }));
  }
}
