// Guards the Python and JavaScript binding surfaces against silent drift.
// It reads the Python source statically, so it runs in the JS test job without a
// Python runtime. See docs/solutions/bindings/binding-parity-guard.md.
import test from 'ava'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const jsSrc = readFileSync(join(here, '..', 'index.js'), 'utf8')
const pySrc = readFileSync(join(here, '..', '..', 'nbos', 'nbos', 'core.py'), 'utf8')

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
    const m = line.match(/^  (?:async )?(?:get |set )?([A-Za-z_]\w*)\s*\(/)
    if (m && m[1] !== 'constructor') members.add(m[1])
  }
  return members
}

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
    const m = line.match(/^    (?:async )?def (\w+)/)
    if (m && m[1] !== '__init__') members.add(m[1])
  }
  return members
}

const snake = (name) => name.replace(/[A-Z]/g, (ch) => '_' + ch.toLowerCase())
const normJs = (set) =>
  new Set([...set].map(snake).filter((name) => !name.startsWith('_')))
const normPy = (set) =>
  new Set(
    [...set]
      .filter((name) => !name.startsWith('_'))
      .map((name) => (name.startsWith('with_') ? name.slice(5) : name)),
  )

function assertNoDrift(t, label, js, py, knownJsOnly, knownPyOnly) {
  const jsOnly = [...js].filter((n) => !py.has(n) && !knownJsOnly.has(n)).sort()
  const pyOnly = [...py].filter((n) => !js.has(n) && !knownPyOnly.has(n)).sort()
  t.deepEqual(jsOnly, [], label + ': JS members missing from Python: ' + jsOnly.join(', '))
  t.deepEqual(pyOnly, [], label + ': Python members missing from JS: ' + pyOnly.join(', '))
}

test('SessionManager surface matches across bindings', (t) => {
  assertNoDrift(
    t,
    'SessionManager',
    normJs(jsMembers(jsSrc, 'SessionManager')),
    normPy(pyMembers(pySrc, 'SessionManager')),
    new Set(['import']),
    new Set(),
  )
})

test('high-level agent surface matches across bindings', (t) => {
  const js = normJs(jsMembers(jsSrc, 'AgentBuilder'))
  const py = normPy(new Set([...pyMembers(pySrc, 'Agent'), ...pyMembers(pySrc, 'AgentBuilder')]))
  assertNoDrift(
    t,
    'Agent',
    js,
    py,
    new Set([
      'circuit_breaker',
      'is_running',
      'rate_limit',
      'skills_from_dir',
      'stop',
      'stream_collect',
      'system',
      'with_config',
      'with_tools',
    ]),
    new Set(['chat', 'plugins', 'skills_dir']),
  )
})
