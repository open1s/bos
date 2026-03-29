export type ApiResponse<T> = T;

const API_BASE = (typeof window !== 'undefined' && (import.meta as any).env?.VITE_API_BASE_URL) || '';

export async function apiGet<T>(path: string): Promise<T> {
  const url = `${API_BASE}${path}`;
  const res = await fetch(url, {
    method: 'GET',
    headers: {
      'Accept': 'application/json',
    },
  });
  if (!res.ok) {
    // Try to extract error message from response
    let msg = 'Request failed';
    try {
      const err = await res.json();
      msg = (err?.message) || JSON.stringify(err);
    } catch {
      // fall through - keep default error message
    }
    throw new Error(msg);
  }
  return (await res.json()) as T;
}
