export interface ExtractedContent {
  url: string;
  title: string;
  mainContent: string;
  metadata: Record<string, string>;
}

export class ContentExtractor {
  async extract(url: string): Promise<ExtractedContent | null> {
    try {
      const response = await fetch(url, {
        headers: { 'User-Agent': 'TRIZ-Agent/1.0' },
        signal: AbortSignal.timeout(10000),
      });

      if (!response.ok) return null;

      const html = await response.text();
      const content = this.extractTextFromHtml(html);

      return {
        url,
        title: this.extractTitle(html) || url,
        mainContent: content,
        metadata: {},
      };
    } catch {
      return null;
    }
  }

  private extractTextFromHtml(html: string): string {
    const body = html.replace(/<script[\s\S]*?<\/script>/gi, '');
    const noStyle = body.replace(/<style[\s\S]*?<\/style>/gi, '');
    const noTags = noStyle.replace(/<[^>]+>/g, ' ');
    const decoded = noTags
      .replace(/&nbsp;/g, ' ')
      .replace(/&amp;/g, '&')
      .replace(/&lt;/g, '<')
      .replace(/&gt;/g, '>')
      .replace(/&quot;/g, '"');
    return decoded.replace(/\s+/g, ' ').trim().slice(0, 10000);
  }

  private extractTitle(html: string): string | null {
    const match = html.match(/<title[^>]*>([^<]+)<\/title>/i);
    return match ? match[1].trim() : null;
  }
}
