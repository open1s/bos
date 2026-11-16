// Guards the binding API reference against drift.
//
// docs/api-reference/*.md is the normative table users read, but it lagged the
// wrappers every time a member was added: the session context setters, the
// canonical publishText/publishJson names, and the MCP listers were all real
// members missing from the tables. This reads the wrappers and the reference
// statically, so it runs in the JS test job with no Rust toolchain and no
// native addon.
import test from 'ava'
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const repo = join(here, '..', '..', '..')
const jsSrc = readFileSync(join(here, '..', 'index.js'), 'utf8')
const jsRef = readFileSync(join(repo, 'docs', 'api-reference', 'jsbos-api.md'), 'utf8')
const pyRef = readFileSync(join(repo, 'docs', 'api-reference', 'nbos-api.md'), 'utf8')

// The package spans several modules (core, content, ...), so read each
// file separately and keep class bodies inside their own file.
const pyDir = join(repo, 'crates', 'nbos', 'nbos')
const pySources = readdirSync(pyDir)
  .filter((name) => name.endsWith('.py'))
  .sort()
  .map((name) => readFileSync(join(pyDir, name), 'utf8'))

// Members that are protocol plumbing or private rather than documented
// surface: the async-iterator protocol hook, and anything underscored.
const INTERNAL = new Set(['next'])

function jsClassBody(name) {
  const start = jsSrc.indexOf('class ' + name + ' ')
  if (start < 0) throw new Error('class not found in index.js: ' + name)
  let depth = 0
  let i = jsSrc.indexOf('{', start)
  for (; i < jsSrc.length; i++) {
    if (jsSrc[i] === '{') depth++
    else if (jsSrc[i] === '}') {
      depth--
      if (depth === 0) break
    }
  }
  return jsSrc.slice(start, i)
}

function pyClassBody(name) {
  // Anchor on a word boundary: 'class Agent' must not also match
  // 'class AgentBuilder'. Search module by module so a body never runs
  // into the next file.
  for (const src of pySources) {
    const m = new RegExp('^class ' + name + '\\b', 'm').exec(src)
    if (!m) continue
    const next = /^class /m.exec(src.slice(m.index + 1))
    return next ? src.slice(m.index, m.index + 1 + next.index) : src.slice(m.index)
  }
  throw new Error('class not found in the nbos package: ' + name)
}

// Methods, getters and setters declared one indent level inside the class.
function jsMembers(body) {
  const found = new Set()
  const re = /^\s{2}(?:static\s+)?(?:async\s+)?(?:get\s+|set\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(/gm
  let m
  while ((m = re.exec(body))) if (m[1] !== 'constructor') found.add(m[1])
  return [...found]
}

function pyMembers(body) {
  const found = new Set()
  const re = /^\s+def ([a-z_][a-z0-9_]*)\(/gm
  let m
  while ((m = re.exec(body))) found.add(m[1])
  return [...found]
}

// A member is documented when it appears as a backticked call or property,
// optionally qualified by its class (BusManager.create).
function documented(ref, name) {
  return new RegExp('`(?:[A-Za-z0-9]+\\.)?' + name + '(?:\\(|`)').test(ref)
}

function undocumented(ref, names) {
  return names.filter(
    (n) => !INTERNAL.has(n) && !n.startsWith('_') && !documented(ref, n)
  )
}

for (const name of [
  'SessionManager',
  'AgentWrapperClass',
  'PublisherWrapper',
  'SubscriberWrapper',
  'BusManager',
  'AgentBuilder',
  'ToolResult',
  'BaseTool',
  'FunctionTool',
  'Binary',
  'ContentPart',
  'Content',
  'Memory',
]) {
  test('jsbos-api.md documents every member of ' + name, (t) => {
    const missing = undocumented(jsRef, jsMembers(jsClassBody(name)))
    t.deepEqual(missing, [], name + ' has undocumented member(s): ' + missing.join(', '))
  })
}

for (const name of [
  'SessionManager',
  'ToolRegistry',
  'AgentBuilder',
  'Agent',
  'Binary',
  'ContentPart',
  'Content',
  'Memory',
]) {
  test('nbos-api.md documents every member of ' + name, (t) => {
    const missing = undocumented(pyRef, pyMembers(pyClassBody(name)))
    t.deepEqual(missing, [], name + ' has undocumented member(s): ' + missing.join(', '))
  })
}