export interface SearchResult {
  title: string;
  url: string;
  snippet: string;
  sourceType: import('./external_reference.js').ReferenceSourceType;
  publishedDate?: string;
  authors?: string[];
}

export interface SearchQuery {
  keywords: string[];
  sourceTypes: Array<import('./external_reference.js').ReferenceSourceType>;
  maxResults?: number;
  language?: string;
}

export interface SearchService {
  search(query: SearchQuery): Promise<SearchResult[]>;
  searchPatents(query: string, maxResults?: number): Promise<SearchResult[]>;
  searchPapers(query: string, maxResults?: number): Promise<SearchResult[]>;
  searchTechSolutions(query: string, maxResults?: number): Promise<SearchResult[]>;
}
