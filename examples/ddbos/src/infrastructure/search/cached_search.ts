import { SearchResult, SearchQuery, SearchService } from '../../domain/solution/search_port.js';
import { ReferenceSourceType } from '../../domain/solution/external_reference.js';

export interface CachedSearchResult {
  query: string;
  results: SearchResult[];
  timestamp: number;
  ttlMs: number;
  isFallback: boolean;
}

export class SearchCache {
  private cache: Map<string, CachedSearchResult> = new Map();
  private defaultTtlMs = 5 * 60 * 1000;

  set(query: string, results: SearchResult[], ttlMs?: number, isFallback = false): void {
    this.cache.set(query, {
      query,
      results,
      timestamp: Date.now(),
      ttlMs: ttlMs || this.defaultTtlMs,
      isFallback,
    });
  }

  get(query: string): SearchResult[] | null {
    const entry = this.cache.get(query);
    if (!entry) return null;
    if (Date.now() - entry.timestamp > entry.ttlMs) {
      this.cache.delete(query);
      return null;
    }
    return entry.results;
  }

  isFallback(query: string): boolean {
    const entry = this.cache.get(query);
    return entry?.isFallback || false;
  }

  getAll(): CachedSearchResult[] {
    const now = Date.now();
    const valid: CachedSearchResult[] = [];
    for (const [key, entry] of this.cache) {
      if (now - entry.timestamp <= entry.ttlMs) {
        valid.push(entry);
      } else {
        this.cache.delete(key);
      }
    }
    return valid;
  }

  clear(): void {
    this.cache.clear();
  }

  get size(): number {
    return this.cache.size;
  }
}

export class CachedSearchService implements SearchService {
  private inner: SearchService;
  private cache: SearchCache;

  constructor(inner: SearchService, cache?: SearchCache) {
    this.inner = inner;
    this.cache = cache || new SearchCache();
  }

  getCache(): SearchCache {
    return this.cache;
  }

  async search(query: SearchQuery): Promise<SearchResult[]> {
    const key = JSON.stringify(query);
    const cached = this.cache.get(key);
    if (cached) return cached;

    const results = await this.inner.search(query);
    this.cache.set(key, results);
    return results;
  }

  async searchPatents(query: string, maxResults?: number): Promise<SearchResult[]> {
    const key = `patents:${query}:${maxResults}`;
    const cached = this.cache.get(key);
    if (cached) return cached;

    const results = await this.inner.searchPatents(query, maxResults);
    this.cache.set(key, results);
    return results;
  }

  async searchPapers(query: string, maxResults?: number): Promise<SearchResult[]> {
    const key = `papers:${query}:${maxResults}`;
    const cached = this.cache.get(key);
    if (cached) return cached;

    const results = await this.inner.searchPapers(query, maxResults);
    this.cache.set(key, results);
    return results;
  }

  async searchTechSolutions(query: string, maxResults?: number): Promise<SearchResult[]> {
    const key = `tech:${query}:${maxResults}`;
    const cached = this.cache.get(key);
    if (cached) return cached;

    const results = await this.inner.searchTechSolutions(query, maxResults);
    this.cache.set(key, results);
    return results;
  }

  getCachedPatents(query: string, maxResults = 5): SearchResult[] {
    return this.cache.get(`patents:${query}:${maxResults}`) || [];
  }

  getCachedPapers(query: string, maxResults = 5): SearchResult[] {
    return this.cache.get(`papers:${query}:${maxResults}`) || [];
  }

  getCachedTechSolutions(query: string, maxResults = 5): SearchResult[] {
    return this.cache.get(`tech:${query}:${maxResults}`) || [];
  }

  getCachedPriorArt(query: string, maxResults = 5): SearchResult[] {
    const patents = this.getCachedPatents(query, maxResults);
    const papers = this.getCachedPapers(query, maxResults);
    const tech = this.getCachedTechSolutions(query, maxResults);
    return [...patents, ...papers, ...tech].slice(0, maxResults * 3);
  }
}
