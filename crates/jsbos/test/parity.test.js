// Guards the Python and JavaScript binding surfaces against silent drift.
// It reads both sources statically, so it runs in the JS test job without a
// Python runtime. See docs/solutions/bindings/binding-parity-guard.md.
import test from 'ava'
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const jsSrc = readFileSync(join(here, '..', 'index.js'), 'utf8')
// Read every Python module, not just core.py: the content, tool, bus, query
// and caller surfaces live in sibling modules.
const pyDir = join(here, '..', '..', 'nbos', 'nbos')
const pySrc = readdirSync(pyDir)
  .filter((f) => f.endsWith('.py'))
  .map((f) => readFileSync(join(pyDir, f), 'utf8'))
  .join('\n')

// JS: accept static/async/get/set modifiers. Static methods used to be
// invisible, which hid half of every class's surface from the guard.
function jsMembers(src, className) {
  const members = new Set()
  let inside = false
  for (const line of src.split('\n')) {
    const cls = line.match(/^(?:export )?class (\w+)/)
    if (cls) {
      inside = cls[1] === className
      continue
    }
    if (/^\}/.test(line)) {
      inside = false
      continue
    }
    if (!inside) continue
    const m = line.match(/^  (?:static )?(?:async )?(?:get |set )?([A-Za-z_]\w*)\s*\(/)
    if (m && m[1] !== 'constructor') members.add(m[1])
  }
  return members
}

// Python: a class body ends at the first column-0 statement that is not a
// class. Tracking the dedent keeps module-level helpers (and the functions
// nested inside them) from being attributed to the last class in a file.
function pyMembers(src, className) {
  const members = new Set()
  let inside = false
  for (const line of src.split('\n')) {
    const cls = line.match(/^class (\w+)/)
    if (cls) {
      inside = cls[1] === className
      continue
    }
    if (!inside) continue
    if (line.trim() !== '' && !/^\s/.test(line)) {
      inside = false
      continue
    }
    const m = line.match(/^    (?:async )?def (\w+)/)
    if (m && m[1] !== '__init__') members.add(m[1])
  }
  return members
}

// camelCase to snake_case with acronym runs collapsed: toJSON becomes to_json,
// fromBase64 becomes from_base64, audioURL becomes audio_url.
const snake = (name) =>
  name
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .toLowerCase()
const stripWith = (name) => (name.startsWith('with_') ? name.slice(5) : name)
// Normalize both bindings: camelCase to snake_case, drop the optional with_
// builder prefix on either side, and ignore private members. A withFoo setter
// therefore matches a foo getter; the contract checks presence, not arity.
const norm = (members) =>
  new Set(
    [...members].map(snake).map(stripWith).filter((name) => !name.startsWith('_')),
  )

function assertNoDrift(t, label, js, py, knownJsOnly, knownPyOnly) {
  const jsOnly = [...js].filter((n) => !py.has(n) && !knownJsOnly.has(n)).sort()
  const pyOnly = [...py].filter((n) => !js.has(n) && !knownPyOnly.has(n)).sort()
  t.deepEqual(jsOnly, [], label + ': JS members missing from Python: ' + jsOnly.join(', '))
  t.deepEqual(pyOnly, [], label + ': Python members missing from JS: ' + pyOnly.join(', '))
}

// --- extractor self-tests -------------------------------------------------
// A broken extractor fails open: it simply returns fewer members, and every
// surface looks like it matches. Cover the two ways that used to happen.
test('extractor sees static methods on both sides', (t) => {
  t.true(jsMembers(jsSrc, 'Binary').has('fromBase64'))
  t.true(jsMembers(jsSrc, 'Content').has('text'))
  t.true(pyMembers(pySrc, 'Binary').has('from_base64'))
})

test('extractor does not leak module-level functions into a class', (t) => {
  // def decorator is nested inside the module-level def tool, and used to be
  // attributed to ToolResult, the last class in tool.py.
  t.false(pyMembers(pySrc, 'ToolResult').has('decorator'))
  t.true(pyMembers(pySrc, 'ToolResult').has('success'))
})

test('acronyms normalize to snake_case words', (t) => {
  t.true(norm(jsMembers(jsSrc, 'Content')).has('to_json'))
  t.true(norm(jsMembers(jsSrc, 'Binary')).has('to_json'))
})

// --- surface parity -------------------------------------------------------
test('SessionManager surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'SessionManager',
    norm(jsMembers(jsSrc, 'SessionManager')),
    norm(pyMembers(pySrc, 'SessionManager')),
    new Set(['import']),
    new Set(),
  )
})

test('high-level agent surface matches across bindings', (t) => {
  const js = norm(jsMembers(jsSrc, 'AgentBuilder'))
  const py = norm(new Set([...pyMembers(pySrc, 'Agent'), ...pyMembers(pySrc, 'AgentBuilder')]))
  assertNoDrift(
    t,
    'Agent',
    js,
    py,
    // Agent surface is fully mirrored; only SessionManager keeps an alias.
    new Set(),
    new Set(),
  )
})

// The multimodal wire types and tool definitions. Both sides serialize the
// same shape, but Python returns a dict from to_dict while JS implements
// toJSON, and JS keeps a toString alongside Python's to_json; those two idioms
// are the only documented differences, plus JS's ToolResult.fromResult helper.
test('multimodal and tool value types match across bindings', (t) => {
  const cases = [
    ['Binary', 'Binary', new Set(['to_json']), new Set(['to_dict'])],
    ['ContentPart', 'ContentPart', new Set(['to_json']), new Set(['to_dict'])],
    ['Content', 'Content', new Set(['to_string']), new Set()],
    ['ToolDef', 'ToolDef', new Set(), new Set()],
    ['ToolResult', 'ToolResult', new Set(['from_result']), new Set()],
  ]
  for (const [label, name, jsOnly, pyOnly] of cases) {
    assertNoDrift(
      t,
      label,
      norm(jsMembers(jsSrc, name)),
      norm(pyMembers(pySrc, name)),
      jsOnly,
      pyOnly,
    )
  }
})

test('memory surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'Memory',
    norm(jsMembers(jsSrc, 'Memory')),
    norm(pyMembers(pySrc, 'Memory')),
    new Set(),
    new Set(),
  )
})

test('ToolRegistry surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'ToolRegistry',
    norm(jsMembers(jsSrc, 'ToolRegistry')),
    norm(pyMembers(pySrc, 'ToolRegistry')),
    new Set(),
    new Set(),
  )
})

test('Config surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'Config',
    norm(jsMembers(jsSrc, 'Config')),
    norm(pyMembers(pySrc, 'Config')),
    new Set(),
    // Python keeps the original add_*/load_sync names as backward-compatible
    // aliases of file/directory/inline/load/reload.
    new Set(['add_file', 'add_directory', 'add_inline', 'load_sync', 'reload_sync']),
  )
})

test('BrainOS facade surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'BrainOS',
    norm(jsMembers(jsSrc, 'BrainOS')),
    norm(pyMembers(pySrc, 'BrainOS')),
    new Set(),
    new Set(),
  )
})

test('Publisher and Subscriber surfaces match across bindings', (t) => {
  assertNoDrift(
    t,
    'Publisher',
    norm(jsMembers(jsSrc, 'PublisherWrapper')),
    norm(pyMembers(pySrc, 'Publisher')),
    // JS keeps text/json as deprecated aliases of publishText/publishJson.
    new Set(['text', 'json']),
    new Set(),
  )
  assertNoDrift(
    t,
    'Subscriber',
    norm(jsMembers(jsSrc, 'SubscriberWrapper')),
    norm(pyMembers(pySrc, 'Subscriber')),
    new Set(),
    // Python keeps the original *_with_timeout_ms names as deprecated
    // aliases of recv/recv_json.
    new Set(['recv_with_timeout_ms', 'recv_json_with_timeout_ms']),
  )
})

test('Query, Queryable, Caller and Callable surfaces match across bindings', (t) => {
  assertNoDrift(
    t,
    'Query',
    norm(jsMembers(jsSrc, 'QueryClient')),
    norm(pyMembers(pySrc, 'Query')),
    new Set(),
    new Set(['create', 'query_text', 'query_text_timeout_ms']),
  )
  assertNoDrift(
    t,
    'Queryable',
    norm(jsMembers(jsSrc, 'QueryableServer')),
    norm(pyMembers(pySrc, 'Queryable')),
    new Set(),
    new Set(['create']),
  )
  assertNoDrift(
    t,
    'Caller',
    norm(jsMembers(jsSrc, 'CallerClient')),
    norm(pyMembers(pySrc, 'Caller')),
    new Set(),
    new Set(['create', 'call_text']),
  )
  assertNoDrift(
    t,
    'Callable',
    norm(jsMembers(jsSrc, 'CallableServer')),
    norm(pyMembers(pySrc, 'Callable')),
    new Set(),
    new Set(['create']),
  )
})

test('BusManager surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'BusManager',
    norm(jsMembers(jsSrc, 'BusManager')),
    norm(pyMembers(pySrc, 'BusManager')),
    new Set(),
    // Python keeps the explicit create_*/publish_* names as aliases of the
    // fluent publisher/subscriber/query/queryable/caller/callable and publish.
    new Set([
      'publish_text',
      'publish_json',
      'create_publisher',
      'create_subscriber',
      'create_query',
      'create_queryable',
      'create_caller',
      'create_callable',
    ]),
  )
})
