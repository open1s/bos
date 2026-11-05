// Guards the Ferris binding surfaces across JavaScript and Python.
// Reads both sources statically, so it runs without a native addon.
import test from 'ava'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const root = join(here, '..')
const jsSrc = readFileSync(join(root, 'index.js'), 'utf8')
const jsDts = readFileSync(join(root, 'jsbos.d.ts'), 'utf8')
const pyFerris = readFileSync(join(root, '..', 'nbos', 'src', 'ferris.rs'), 'utf8')
const pyHost = readFileSync(join(root, '..', 'nbos', 'src', 'ferris_host.rs'), 'utf8')

function jsClassMembers(src, className, indent = '  ') {
  const members = new Set()
  let inside = false
  const re = new RegExp('^' + indent + '(?:static\\s+)?(?:async\\s+)?(?:get\\s+|set\\s+)?([A-Za-z_]\\w*)\\s*\\(')
  for (const line of src.split('\n')) {
    const cls = line.match(/^(?:export )?class (\w+)/)
    if (cls) {
      inside = cls[1] === className
      continue
    }
    if (inside && /^}/.test(line)) {
      inside = false
      continue
    }
    if (!inside) continue
    const m = line.match(re)
    if (m && m[1] !== 'constructor') members.add(m[1])
  }
  return members
}

function rustPyMembers(src, implName) {
  const start = src.indexOf('#[pymethods]\nimpl ' + implName + ' {')
  if (start < 0) throw new Error('pymethods impl not found: ' + implName)
  let depth = 0
  let i = src.indexOf('{', start)
  for (; i < src.length; i++) {
    if (src[i] === '{') depth++
    else if (src[i] === '}') {
      depth--
      if (depth === 0) break
    }
  }
  const body = src.slice(start, i)
  const members = new Set()
  for (const m of body.matchAll(/^\s*(?:pub\s+)?(?:async\s+)?fn\s+([a-z_]\w*)\s*(?:<[^>]*>)?\s*\(/gm)) {
    if (m[1] !== 'new') members.add(m[1])
  }
  return members
}

function jsDtsMembers(src, className) {
  const start = src.indexOf('export declare class ' + className + ' ')
  if (start < 0) throw new Error('class not found: ' + className)
  let depth = 0
  let i = src.indexOf('{', start)
  for (; i < src.length; i++) {
    if (src[i] === '{') depth++
    else if (src[i] === '}') {
      depth--
      if (depth === 0) break
    }
  }
  const body = src.slice(start, i)
  const members = new Set()
  for (const m of body.matchAll(/^  (?:static\s+)?(?:get\s+)?([A-Za-z_]\w*)\s*[(?:]/gm)) {
    if (m[1] !== 'constructor') members.add(m[1])
  }
  return members
}

const snake = (name) =>
  name
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .toLowerCase()
const norm = (members) =>
  new Set([...members].map(snake).filter((name) => !name.startsWith('__')))

function compare(t, label, js, py) {
  const j = norm(js)
  const p = norm(py)
  const jsOnly = [...j].filter((name) => !p.has(name)).sort()
  const pyOnly = [...p].filter((name) => !j.has(name)).sort()
  t.deepEqual(jsOnly, [], label + ': JS members missing from Python: ' + jsOnly.join(', '))
  t.deepEqual(pyOnly, [], label + ': Python members missing from JS: ' + pyOnly.join(', '))
}

test('FerrisManifest surface matches across bindings', (t) => {
  compare(
    t,
    'FerrisManifest',
    jsDtsMembers(jsDts, 'FerrisManifest'),
    rustPyMembers(pyFerris, 'PyFerrisManifest'),
  )
})

test('FerrisManifestBuilder surface matches across bindings', (t) => {
  compare(
    t,
    'FerrisManifestBuilder',
    jsDtsMembers(jsDts, 'FerrisManifestBuilder'),
    rustPyMembers(pyFerris, 'PyFerrisManifestBuilder'),
  )
})

test('FerrisHarness surface matches across bindings', (t) => {
  compare(
    t,
    'FerrisHarness',
    jsClassMembers(jsSrc, 'FerrisHarness'),
    rustPyMembers(pyFerris, 'PyFerrisHarness'),
  )
})

test('FerrisHost surface matches across bindings', (t) => {
  // register_agent_tools lives in the Python wrapper (nbos/__init__.py), not
  // in the native pyclass; it mirrors the JS wrapper helper.
  const py = rustPyMembers(pyHost, 'PyFerrisHost')
  py.add('register_agent_tools')
  compare(t, 'FerrisHost', jsClassMembers(jsSrc, 'FerrisHost'), py)
})
