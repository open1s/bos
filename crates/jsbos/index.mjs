/**
 * @open1s/jsbos - ESM wrapper
 * Re-exports all exports from the CJS modules for ESM compatibility
 */

import { createRequire } from 'module';
const require = createRequire(import.meta.url);

// Load the CJS index.js (NAPI-RS bindings)
const index = require('./index.js');

// Load the CJS brainos.js (high-level API)
const brainos = require('./brainos.js');

// Re-export everything
export const {
  Agent,
  AgentCallableServer,
  AgentRpcClient,
  Bus,
  Callable,
  Caller,
  ConfigLoader,
  HookRegistry,
  McpClient,
  PluginRegistry,
  Publisher,
  Query,
  Queryable,
  Subscriber,
  HookDecision,
  HookEvent,
  initTracing,
  logTestMessage,
  PluginStage,
  version,
} = index;

// Re-export high-level API from brainos.js
export * from './brainos.js';