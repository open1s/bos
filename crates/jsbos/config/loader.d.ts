/**
 * TypeScript declarations for ESM wrapper of ConfigLoader
 */

export interface ConfigLoader {
  discover(): void;
  addFile(path: string): void;
  addDirectory(path: string): void;
  addInline(data: Record<string, unknown>): void;
  reset(): void;
  loadSync(): string;
  reloadSync(): string;
}

export type ConfigLoaderConstructor = new () => ConfigLoader;

declare const ConfigLoader: ConfigLoaderConstructor;

export { ConfigLoader };
