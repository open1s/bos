// Guards the wrapper-to-native seam: every method a high-level wrapper calls on
// its native object must exist in the generated binding interface. Both sides
// are read statically, so it runs in the JS test job without a Python runtime.
import test from 'ava'
import { readFileSync, readdirSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'

const here = dirname(fileURLToPath(import.meta.url))
const jsbosDir = join(here, '..')
const nbosDir = join(here, '..', '..', 'nbos')

// camelCase to snake_case with acronym runs collapsed: publishText becomes
// publish_text, addMcpServerHttp becomes add_mcp_server_http.
const snake = (name) =>
  name
    .replace(/([a-z0-9])([A-Z])/g, '$1_$2')
    .replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2')
    .toLowerCase()

// JS native members: methods and fields declared in the generated typings.
function jsNativeMembers() {
  const src = readFileSync(join(jsbosDir, 'jsbos.d.ts'), 'utf8')
  const members = new Set()
  for (const line of src.split('\n')) {
    const m = line.match(/^  (?:static\s+)?(?:get\s+)?([A-Za-z_]\w*)\s*[(?:]/)
    if (m && m[1] !== 'constructor') members.add(m[1])
  }
  return members
}

// Python native members: functions declared inside any #[pymethods] impl block.
function pyNativeMembers() {
  const members = new Set()
  for (const file of readdirSync(join(nbosDir, 'src'))) {
    if (!file.endsWith('.rs')) continue
    let inMethods = false
    for (const line of readFileSync(join(nbosDir, 'src', file), 'utf8').split('\n')) {
      if (/^\s*#\[pymethods\]/.test(line)) {
        inMethods = true
        continue
      }
      if (inMethods && /^}\s*$/.test(line)) {
        inMethods = false
        continue
      }
      if (!inMethods) continue
      const m = line.match(/^\s*(?:pub\s+)?(?:async\s+)?fn\s+([a-z_]\w*)/)
      if (m) members.add(m[1])
    }
  }
  return members
}

// Names passed to `object.method(` calls that target the wrapper native field.
function nativeCalls(src, pattern) {
  const calls = new Set()
  for (const line of src.split('\n')) {
    for (const m of line.matchAll(pattern)) calls.add(m[1])
  }
  return calls
}

test('every JS wrapper call on _inner resolves to a native member', (t) => {
  const members = jsNativeMembers()
  const memberSnake = new Set([...members].map(snake))
  const src = readFileSync(join(jsbosDir, 'index.js'), 'utf8')
  const calls = nativeCalls(src, /this\._inner\.([A-Za-z_]\w*)\s*\(/g)
  t.true(calls.size > 20, 'expected the wrapper to call the native object')
  const missing = [...calls].filter((c) => !memberSnake.has(snake(c))).sort()
  t.deepEqual(missing, [], 'JS wrapper calls with no native member: ' + missing.join(', '))
})

test('every Python wrapper call on _inner resolves to a native member', (t) => {
  const members = pyNativeMembers()
  const calls = new Set()
  for (const file of readdirSync(join(nbosDir, 'nbos'))) {
    if (!file.endsWith('.py')) continue
    const src = readFileSync(join(nbosDir, 'nbos', file), 'utf8')
    for (const c of nativeCalls(src, /self\._inner\.([a-z_]\w*)\s*\(/g)) calls.add(c)
  }
  t.true(calls.size > 20, 'expected the wrapper to call the native object')
  const missing = [...calls].filter((c) => !members.has(c)).sort()
  t.deepEqual(missing, [], 'Python wrapper calls with no native member: ' + missing.join(', '))
})

// Extractor self-tests: the parsers must actually see the surfaces they guard.
test('native extractors see known members', (t) => {
  t.true(jsNativeMembers().has('registerHook'))
  t.true(jsNativeMembers().has('publishText'))
  t.true(pyNativeMembers().has('register_hook'))
  t.true(pyNativeMembers().has('publish_text'))
})