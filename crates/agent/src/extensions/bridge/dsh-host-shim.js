'use strict';
/**
 * BOS DSH host shim.
 *
 * Runs ONE DeepSeek Harness plugin host entry (`apply(ctx, config)`) inside a
 * Node subprocess and reports every registration it makes back to the BOS
 * extension substrate over a line-delimited JSON-RPC protocol on stdio.
 *
 * The shim implements only the parts of the DSH host API that a registration
 * can be carried by. It is NOT the DSH runtime: a plugin that needs a real
 * host service (ctx.fs, ctx.subprocess, ctx.llm's transport, ctx.settings,
 * ctx.webServer, the Cordis loader) gets a seam that either refuses by name or
 * returns an empty, honest value. Refusing a seam is the point: a plugin that
 * reads `ctx.fs` and finds nothing fails loudly at load instead of silently
 * contributing half a capability.
 *
 * Nothing here is a security boundary. The subprocess runs with the user's
 * privileges; see docs/extension-standards.md. What protects the user is the
 * trust gate in the host, which decides whether this file is started at all.
 *
 * Protocol (one JSON object per line, in both directions).
 *
 * host -> shim:
 *   {"id":N,"method":"load","params":{"entry":"<abs path>","config":{...},"cwd":"<dir>"}}
 *   {"id":N,"method":"invoke","params":{"registration":"<id>","input":{...}}}
 *   {"id":N,"method":"shutdown"}
 *
 * shim -> host:
 *   {"type":"hello","protocol":1}
 *   {"type":"register","registration":"<id>","kind":"tool","name":"...","description":"...",
 *    "schema":{...}}                       // tools
 *   {"type":"register", ..., "kind":"command", "name","description","input":{...}|null}
 *   {"type":"register", ..., "kind":"skill", "name","description","invocation":{...}}
 *   {"type":"register", ..., "kind":"hook", "name":"<event>", "stages":[...]}
 *   {"type":"register", ..., "kind":"context", "name":"<event>"}   // an event listener
 *   {"type":"refuse","kind":"<seam>","name":"<detail>","reason":"<sentence>"}
 *   {"type":"log","level":"info|warn|error","message":"..."}
 *   {"type":"ready"}
 *   {"type":"error","message":"...","stack":"..."}
 *   {"type":"result","id":N,"ok":true,"value":{...}}
 *   {"type":"result","id":N,"ok":false,"error":"..."}
 */

const path = require('node:path');
const fs = require('node:fs');
const { pathToFileURL } = require('node:url');

/** Protocol revision the host and this file agree on. */
const PROTOCOL = 1;

/** Registrations this shim has announced, keyed by id, never by name. */
const registrations = new Map();
let nextRegistration = 0;
/** Set once `load` has been answered, so a late async registration is refused. */
let loaded = false;
let closing = false;

/** Write one protocol frame. Serialised: concurrent writes must not interleave. */
function emit(frame) {
  if (closing) return;
  process.stdout.write(JSON.stringify(frame) + '\n');
}

/** Announce one registration and mint its id. */
function announce(kind, fields) {
  const registration = 'r' + nextRegistration++;
  registrations.set(registration, { kind, name: fields.name });
  emit(Object.assign({ type: 'register', registration, kind }, fields));
  return registration;
}

/** Report a seam the plugin asked for that this shim cannot honour. */
function refuse(kind, name, reason) {
  emit({ type: 'refuse', kind, name, reason });
}

/** Forward a plugin log line. Bodies are strings, never objects. */
function log(level, message) {
  emit({ type: 'log', level, text: String(message) });
}

/** A disposer that does nothing, in the shape Cordis and DSH both expect. */
function noopDisposer() {
  return () => {};
}

/**
 * Build the `ctx.logger` a DSH plugin reads.
 *
 * The real logger is Cordis's; a plugin uses it for diagnostics only, so every
 * level forwards to the host and nothing else changes.
 */
function makeLogger() {
  const at = (level) => (...args) => {
    const parts = args.map((arg) =>
      typeof arg === 'string' ? arg : safeString(arg),
    );
    log(level, parts.join(' '));
    return undefined;
  };
  return {
    error: at('error'),
    warn: at('warn'),
    info: at('info'),
    debug: at('debug'),
    trace: at('trace'),
    level: () => 'info',
    success: at('info'),
    fail: at('error'),
  };
}

function safeString(value) {
  if (value instanceof Error) return value.message;
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

/** Normalise a DSH tool definition into the fields the host stores. */
function toolRegistration(definition) {
  if (!definition || typeof definition !== 'object') {
    throw new TypeError('a tool registration must be an object');
  }
  const name = definition.name;
  if (typeof name !== 'string' || name.length === 0) {
    throw new TypeError('a tool registration needs a non-empty name');
  }
  if (typeof definition.description !== 'string') {
    throw new TypeError('tool "' + name + '" needs a description');
  }
  const parameters =
    definition.parameters && typeof definition.parameters === 'object'
      ? definition.parameters
      : { type: 'object', properties: {} };
  // The real defineTool() pairs every tool with an output renderer. The render
  // is a UI concern the host does not have, so it is accepted and dropped; the
  // tool's own callable surface is name/description/parameters/execute.
  return {
    name,
    description: definition.description,
    schema: parameters,
    hasExecute: typeof definition.execute === 'function',
  };
}

/** Build `ctx.tools`. */
function makeTools() {
  return {
    register(definition) {
      const fields = toolRegistration(definition);
      if (!fields.hasExecute) {
        // Registered with no body: honest to accept, but the host must know the
        // tool cannot be called, or a model would be offered a name that fails.
        refuse(
          'tool',
          fields.name,
          'the plugin registered tool "' +
            fields.name +
            '" without an execute function, so it has no callable body',
        );
        return noopDisposer();
      }
      const registration = announce('tool', {
        name: fields.name,
        description: fields.description,
        schema: fields.schema,
      });
      registrations.get(registration).execute = definition.execute;
      return () => {
        registrations.delete(registration);
      };
    },
    /** `registerTool` is the same seam under an older name. */
    registerTool(definition) {
      return this.register(definition);
    },
    get(name) {
      for (const [, entry] of registrations) {
        if (entry.kind === 'tool' && entry.name === name) return entry;
      }
      return undefined;
    },
    list() {
      const out = [];
      for (const [, entry] of registrations) {
        if (entry.kind === 'tool') out.push(entry.name);
      }
      return out;
    },
  };
}

/** Build `ctx.commands`. */
function makeCommands() {
  return {
    register(definition) {
      if (!definition || typeof definition !== 'object') {
        throw new TypeError('a command registration must be an object');
      }
      const name = definition.name;
      if (typeof name !== 'string' || !/^[a-z0-9][a-z0-9-]*$/.test(name)) {
        throw new TypeError(
          'command name "' + name + '" must be lower-case kebab-case',
        );
      }
      if (typeof definition.description !== 'string' || !definition.description.trim()) {
        throw new TypeError('command "' + name + '" needs a description');
      }
      if (typeof definition.handler !== 'function') {
        throw new TypeError('command "' + name + '" needs a handler');
      }
      const input =
        definition.input && typeof definition.input === 'object'
          ? { hint: String(definition.input.hint || '') }
          : null;
      const registration = announce('command', {
        name,
        description: definition.description,
        input,
      });
      registrations.get(registration).execute = (args) =>
        definition.handler(args && typeof args === 'object' ? args : {});
      return () => {
        registrations.delete(registration);
      };
    },
    list() {
      const out = [];
      for (const [, entry] of registrations) {
        if (entry.kind === 'command') out.push(entry.name);
      }
      return out;
    },
  };
}

/** Build `ctx.skills`. */
function makeSkills() {
  return {
    register(skill) {
      if (!skill || typeof skill !== 'object') {
        throw new TypeError('a skill registration must be an object');
      }
      const name = skill.name;
      if (typeof name !== 'string' || name.length === 0) {
        throw new TypeError('a skill registration needs a name');
      }
      if (typeof skill.description !== 'string' || skill.description.length === 0) {
        throw new TypeError('skill "' + name + '" needs a description');
      }
      const invocation = skill.invocation || {
        modelInvocable: true,
        userInvocable: true,
      };
      const registration = announce('skill', {
        name,
        description: skill.description,
        invocation: {
          modelInvocable: invocation.modelInvocable !== false,
          userInvocable: invocation.userInvocable !== false,
        },
        // A runtime skill's body is carried in `content`; the host registers it
        // as a prompt-backed skill when it is present.
        content: typeof skill.content === 'string' ? skill.content : null,
        whenToUse: typeof skill.whenToUse === 'string' ? skill.whenToUse : null,
      });
      return () => {
        registrations.delete(registration);
      };
    },
    registerProvider(create) {
      // A provider borrows the skill catalog of a same-process service. This
      // subprocess has no catalog to borrow, and pretending one is empty would
      // make a plugin's skills silently disappear.
      refuse(
        'skills',
        'registerProvider',
        'ctx.skills.registerProvider supplies a same-process skill catalog; this bridge runs the plugin out of process and has no catalog to borrow',
      );
      void create;
      return noopDisposer();
    },
  };
}

/**
 * The DSH event names that map onto a real BOS hook stage.
 *
 * Everything else is a Cordis event this host does not dispatch; listening to
 * it is recorded as a context registration and reported as a refusal so the
 * reader is never told a hook is active when nothing will call it.
 */
const HOOK_EVENTS = {
  'llm/request': 'llm_request',
  'llm/response': 'llm_response',
  'llm/stream-token': 'stream_token',
  'tool/call': 'tool_call',
  'tool/result': 'tool_result',
};

/** Build `ctx.on` and the Cordis event bus it belongs to. */
function makeEvents() {
  const listeners = new Map();
  const on = (name, listener, options) => {
    if (typeof name !== 'string' || typeof listener !== 'function') {
      throw new TypeError('ctx.on(name, listener) needs a name and a function');
    }
    if (!listeners.has(name)) listeners.set(name, []);
    const list = listeners.get(name);
    if (options && options.prepend) list.unshift(listener);
    else list.push(listener);
    const stage = HOOK_EVENTS[name];
    if (stage) {
      const registration = announce('hook', { name, stages: [stage] });
      registrations.get(registration).execute = (args) => {
        for (const fn of listeners.get(name) || []) {
          const result = fn(args);
          if (result !== undefined && result !== null && result !== false) {
            return result;
          }
        }
        return undefined;
      };
      return () => {
        registrations.delete(registration);
      };
    }
    refuse(
      'event',
      name,
      'the plugin listens for "' +
        name +
        '", a DeepSeek Harness host event this bridge does not dispatch; the listener is recorded but will never be called',
    );
    return () => {
      const list2 = listeners.get(name) || [];
      const at = list2.indexOf(listener);
      if (at >= 0) list2.splice(at, 1);
    };
  };
  return {
    on,
    once(name, listener, options) {
      const wrapped = (...args) => {
        off();
        return listener(...args);
      };
      const off = on(name, wrapped, options);
      return off;
    },
    off(name, listener) {
      const list = listeners.get(name);
      if (!list) return false;
      const at = list.indexOf(listener);
      if (at < 0) return false;
      list.splice(at, 1);
      return true;
    },
    emit(name, ...args) {
      // Cordis's fire-and-forget dispatch. Listeners already announced are
      // called; nothing else is dispatched.
      for (const fn of listeners.get(name) || []) fn(...args);
      return true;
    },
    /** Cascade is Cordis's shorthand for a chain of `on` calls. */
    cascade() {
      refuse(
        'event',
        'cascade',
        'ctx.cascade registers a chain of Cordis events this bridge does not dispatch',
      );
      return noopDisposer();
    },
  };
}

/** Services a plugin may look up by name and that this bridge genuinely has. */
function makeGet(ctx) {
  const known = {
    tools: () => ctx.tools,
    commands: () => ctx.commands,
    skills: () => ctx.skills,
    logger: () => ctx.logger,
  };
  return (name) => {
    const hit = known[name];
    if (hit) return hit();
    refuse(
      'service',
      String(name),
      'the plugin resolved host service "' +
        name +
        '", which this bridge does not provide; it is undefined here rather than a stub, so the plugin can fail loudly',
    );
    return undefined;
  };
}

/**
 * Build the `ctx` a DSH plugin's `apply` receives.
 *
 * Basic services this bridge can offer are real; everything that would need a
 * live host (filesystem, subprocess, credentials, settings, the LLM transport,
 * the Cordis loader, the UI slot tree) is a REFUSING seam: the call throws a
 * named error rather than returning an empty stub, so a plugin cannot quietly
 * contribute half of what it declares.
 */
function makeContext(config) {
  const ctx = {};
  const refused = (seam, detail) => {
    refuse('service', seam, detail);
    const error = new Error('dsh-bridge: ' + detail);
    error.name = 'DshBridgeUnsupportedService';
    return error;
  };
  const refusing = (seam, detail) =>
    new Proxy(
      {},
      {
        get() {
          throw refused(seam, detail);
        },
        apply() {
          throw refused(seam, detail);
        },
      },
    );

  ctx.logger = makeLogger();
  ctx.tools = makeTools();
  ctx.commands = makeCommands();
  ctx.skills = makeSkills();
  const events = makeEvents();
  ctx.on = events.on;
  ctx.once = events.once;
  ctx.off = events.off;
  ctx.emit = events.emit;
  ctx.cascade = events.cascade;
  ctx.events = events;
  ctx.get = makeGet(ctx);
  ctx.config = config && typeof config === 'object' ? config : {};
  ctx.effect = (execute, label) => {
    const value = typeof execute === 'function' ? execute() : undefined;
    if (typeof value === 'function') return value;
    log('debug', 'ctx.effect(' + (label || 'anonymous') + ') installed');
    return noopDisposer();
  };
  ctx.inject = (services, callback) => {
    const wanted = Array.isArray(services) ? services : [services];
    const available = wanted.filter((name) =>
      ['tools', 'commands', 'skills', 'logger', 'events'].includes(name),
    );
    if (typeof callback === 'function') {
      const scoped = Object.create(ctx);
      for (const name of available) scoped[name] = ctx[name];
      try {
        callback(scoped);
      } catch (error) {
        log('warn', 'ctx.inject callback failed: ' + safeString(error));
      }
    }
    for (const name of wanted) {
      if (!available.includes(name)) {
        refuse(
          'service',
          'inject:' + name,
          'the plugin injects host service "' +
            name +
            '", which this bridge does not provide',
        );
      }
    }
    return noopDisposer();
  };
  ctx.provide = (name) =>
    refusing(
      'provide:' + String(name),
      'the plugin publishes host service "' +
        name +
        '"; this bridge is one plugin in one subprocess and provides no service container',
    );

  // Refusing seams. Each names itself in the error and in a refusal frame.
  ctx.fs = refusing(
    'fs',
    'ctx.fs is a DeepSeek Harness host filesystem service; this bridge runs the plugin in a subprocess with the ordinary Node fs module and does not proxy the host filesystem',
  );
  ctx.subprocess = refusing(
    'subprocess',
    'ctx.subprocess is a DeepSeek Harness host process service; this bridge does not proxy it',
  );
  ctx.llm = refusing(
    'llm',
    'ctx.llm is the DeepSeek Harness model service; this bridge publishes model providers declaratively (providers/*.json) instead of proxying a live LLM adapter',
  );
  ctx.settings = refusing(
    'settings',
    'ctx.settings is the DeepSeek Harness settings service; this bridge has no settings document to read or write',
  );
  ctx.credentials = refusing(
    'credentials',
    'ctx.credentials is the DeepSeek Harness credential vault; a plugin never receives a credential through this bridge',
  );
  ctx.slots = refusing(
    'slots',
    'ctx.slots is the DeepSeek Harness client UI slot tree; this host has no DSH client renderer',
  );
  ctx.loader = refusing(
    'loader',
    'ctx.loader is the Cordis plugin loader; this bridge loads exactly one entry file',
  );
  ctx.fiber = refusing(
    'fiber',
    'ctx.fiber is the Cordis fiber; this bridge runs one plugin in one process with no disposable tree',
  );
  ctx.reflect = refusing(
    'reflect',
    'ctx.reflect is the Cordis reflective proxy; this bridge exposes a plain context object',
  );
  ctx.ptcRuntime = refusing(
    'ptcRuntime',
    'ctx.ptcRuntime is the DeepSeek Harness PTC runtime service, which this host does not compose',
  );
  ctx.session = refusing(
    'session',
    'ctx.session is the DeepSeek Harness session service; this bridge has no live session',
  );
  ctx.sessions = refusing(
    'sessions',
    'ctx.sessions is the DeepSeek Harness session registry; this bridge has no live session',
  );
  ctx.agents = refusing(
    'agents',
    'ctx.agents is the DeepSeek Harness agent registry; this bridge has no live agent',
  );
  ctx.server = refusing(
    'server',
    'ctx.server is the DeepSeek Harness server service; this bridge composes none',
  );
  return ctx;
}

/** Resolve the entry file the host named, refusing anything outside the package. */
function resolveEntry(entry, root) {
  if (typeof entry !== 'string' || entry.length === 0) {
    throw new Error('load needs an entry path');
  }
  if (!fs.existsSync(entry)) {
    throw new Error('the entry ' + entry + ' does not exist');
  }
  // Both sides are resolved through the real filesystem before comparing, or a
  // symlinked scratch directory (macOS /var -> /private/var) reads as outside
  // its own package.
  const resolved = fs.realpathSync(path.resolve(entry));
  const realRoot = fs.realpathSync(path.resolve(root));
  if (resolved !== realRoot && !resolved.startsWith(realRoot + path.sep)) {
    throw new Error('the entry ' + resolved + ' is outside the package ' + realRoot);
  }
  return resolved;
}

/** How a plugin module exposes `apply`. */
function pickApply(module) {
  if (typeof module === 'function') return module;
  if (module && typeof module.apply === 'function') return module.apply;
  if (module && module.default) {
    if (typeof module.default === 'function') return module.default;
    if (typeof module.default.apply === 'function') return module.default.apply;
  }
  return null;
}

/** Run one `load` request. */
async function handleLoad(params) {
  if (loaded) throw new Error('load was already called on this process');
  const root = params.root || process.cwd();
  const entry = resolveEntry(params.entry, root);
  const config = params.config && typeof params.config === 'object' ? params.config : {};
  const ctx = makeContext(config);

  // The entry is imported from a file URL so both CJS and ESM packages load;
  // an ESM `import` of a .js file inside a "type":"module" package works, and a
  // CJS file with module.exports works through the interop default.
  let module;
  try {
    module = await import(pathToFileURL(entry).href);
  } catch (error) {
    throw new Error(
      'the host entry could not be imported: ' + safeString(error),
    );
  }
  const apply = pickApply(module);
  if (typeof apply !== 'function') {
    throw new Error(
      'the host entry exports no apply(ctx, config) function; a DeepSeek Harness package must export one',
    );
  }
  // apply() may return a promise; a rejection here is a load failure, not a
  // partial success, and the host turns it into a named refusal.
  const result = await apply(ctx, config);
  if (result && typeof result === 'object' && result.dispose) {
    // A returned dispose handle is honoured at shutdown.
    shutdowns.push(result.dispose);
  }
  loaded = true;
}

/** Disposers a plugin returned from `apply`, run in reverse at shutdown. */
const shutdowns = [];

/** Run one `invoke` request: call a registered tool or command. */
async function handleInvoke(params) {
  const entry = registrations.get(params.registration);
  if (!entry) {
    throw new Error(
      'registration "' + params.registration + '" is not registered',
    );
  }
  if (typeof entry.execute !== 'function') {
    throw new Error(
      'registration "' + entry.name + '" has no callable body',
    );
  }
  const input = params.input === undefined ? {} : params.input;
  return await entry.execute(input);
}

/** Read one line-delimited frame at a time; never buffer the whole stream. */
function makeReader(onFrame) {
  let buffer = '';
  process.stdin.setEncoding('utf8');
  process.stdin.on('data', (chunk) => {
    buffer += chunk;
    let index;
    while ((index = buffer.indexOf('\n')) >= 0) {
      const line = buffer.slice(0, index);
      buffer = buffer.slice(index + 1);
      if (!line.trim()) continue;
      let frame;
      try {
        frame = JSON.parse(line);
      } catch {
        emit({
          type: 'result',
          id: null,
          ok: false,
          error: 'the host sent a frame that is not JSON',
        });
        continue;
      }
      onFrame(frame);
    }
    // A plugin that floods stdout must not grow this buffer without bound.
    if (buffer.length > MAX_BUFFER) {
      closing = true;
      emit({
        type: 'result',
        id: null,
        ok: false,
        error: 'the shim read a frame longer than the ' + MAX_BUFFER + ' byte limit',
      });
      process.exit(3);
    }
  });
}

/** Longest single protocol frame either side accepts. */
const MAX_BUFFER = 4 * 1024 * 1024;

async function main() {
  emit({ type: 'hello', protocol: PROTOCOL });
  makeReader((frame) => {
    void handleFrame(frame);
  });
}

async function handleFrame(frame) {
  const id = frame && frame.id;
  const method = frame && frame.method;
  const params = (frame && frame.params) || {};
  try {
    let value;
    if (method === 'load') {
      await handleLoad(params);
      value = { registered: registrations.size };
      loaded = true;
      emit({ type: 'ready' });
    } else if (method === 'invoke') {
      value = { result: await handleInvoke(params) };
    } else if (method === 'shutdown') {
      emit({ type: 'result', id, ok: true, value: { closing: true } });
      process.exit(0);
      return;
    } else {
      throw new Error('unknown method "' + method + '"');
    }
    emit({ type: 'result', id, ok: true, value });
  } catch (error) {
    emit({
      type: 'result',
      id,
      ok: false,
      error: safeString(error),
      stack: error && error.stack ? String(error.stack).slice(0, 4000) : undefined,
    });
    if (method === 'load') {
      // A failed load is terminal: the host must not see a half-loaded plugin.
      emit({ type: 'error', message: safeString(error) });
    }
  }
}

process.on('uncaughtException', (error) => {
  emit({ type: 'error', message: 'uncaught: ' + safeString(error) });
  process.exit(4);
});
process.on('unhandledRejection', (reason) => {
  emit({ type: 'error', message: 'unhandled rejection: ' + safeString(reason) });
  process.exit(5);
});
process.on('SIGTERM', () => {
  closing = true;
  process.exit(0);
});

main().catch((error) => {
  emit({ type: 'error', message: safeString(error) });
  process.exit(6);
});
