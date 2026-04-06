/**
 * ESM wrapper for ConfigLoader
 * Re-exports from the NAPI-RS CommonJS bindings
 */
import { createRequire } from 'module';
import { fileURLToPath } from 'url';
import { dirname, join } from 'path';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);
const require = createRequire(import.meta.url);

// Import from parent directory's CommonJS module
const { ConfigLoader } = require(join(__dirname, '..', 'jsbos.js'));

export { ConfigLoader };
