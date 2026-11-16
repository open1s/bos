// Tests for the Ferris SDK authoring binding: JavaScript extensions implement
// the SDK's Extension trait through callbacks and run under FerrisHarness.
import test from 'ava'
import { FerrisHarness, FerrisManifest, FerrisManifestBuilder } from '../index.js'

const MANIFEST = `
schema_version = "2.0"

[extension]
id = "com.example.echo"
name = "Echo"
version = "1.0.0"
description = "Echoes its input"

[compatibility]
host = ">=3.0.0"
extension_api = ">=1.0.0"

[runtime]
kind = "builtin"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"

[contribution.payload]
description = "Echo"
`

const START = async (ctx) => {
  if (!ctx.id || !ctx.root) throw new Error('missing start context')
  return [{ kind: 'tool', id: 'echo', payload: { description: 'Echo' } }]
}
const INVOKE = async (call) => call.input

test('a manifest parses, re-renders and exposes its identity', (t) => {
  const manifest = FerrisManifest.parse(MANIFEST)
  t.is(manifest.id, 'com.example.echo')
  t.is(manifest.runtimeKind, 'builtin')
  t.true(manifest.toToml().includes('[[contribution]]'))
  const json = manifest.toJSON()
  t.is(json.extension.id, 'com.example.echo')
  t.is(json.runtime.kind, 'builtin')
})

test('the chainable builder produces a valid manifest', (t) => {
  const manifest = new FerrisManifestBuilder(
    'com.example.built',
    'Built',
    '1.2.3',
    'A built manifest',
  )
    .host('>=3.0.0')
    .runtime('declarative')
    .capability('agent.tools.register')
    .contribution('tool', 'echo', { description: 'Echo' })
    .author('Ferris')
    .license('MIT')
    .maxMemoryMb(64)
    .maxExecutionMs(5000)
    .startup('on-demand')
    .build()

  t.is(manifest.id, 'com.example.built')
  t.is(manifest.runtimeKind, 'declarative')
  const toml = manifest.toToml()
  t.true(toml.includes('com.example.built'))
  t.true(toml.includes('agent.tools.register'))
  t.true(toml.includes('max_memory_mb = 64'))
})

test('an invalid manifest is refused at construction', (t) => {
  t.throws(() => FerrisManifest.parse('schema_version = "1.0"'))
  t.throws(() => new FerrisHarness('not toml', '/tmp/ferris-js', START, INVOKE))
})

test('a JavaScript extension starts, invokes and shuts down', async (t) => {
  let shutDown = false
  const harness = new FerrisHarness(
    MANIFEST,
    '/tmp/ferris-js-test',
    START,
    INVOKE,
    async () => {
      shutDown = true
    },
  )

  t.is(harness.runtimeKind, 'builtin')
  t.is(harness.root, '/tmp/ferris-js-test')
  t.is(harness.manifest().extension.id, 'com.example.echo')

  const contributions = await harness.start()
  t.deepEqual(contributions, [
    { kind: 'tool', id: 'echo', payload: { description: 'Echo' } },
  ])

  t.deepEqual(await harness.invoke('echo', { text: 'hi' }), { text: 'hi' })
  t.deepEqual(
    await harness.invokeContribution('tool', 'echo', { nested: [1, 2] }),
    { nested: [1, 2] },
  )

  await harness.shutdown()
  t.true(shutDown)
})

test('sync callbacks work too', async (t) => {
  const harness = new FerrisHarness(
    MANIFEST,
    '/tmp/ferris-js-sync',
    () => [{ kind: 'tool', id: 'echo', payload: {} }],
    (call) => call.input,
  )
  const contributions = await harness.start()
  t.is(contributions.length, 1)
  t.deepEqual(await harness.invoke('echo', { ok: true }), { ok: true })
  await harness.shutdown()
})

test('a callback error surfaces as a rejection', async (t) => {
  const harness = new FerrisHarness(
    MANIFEST,
    '/tmp/ferris-js-error',
    START,
    async () => {
      throw new Error('invoke failed')
    },
  )
  await harness.start()
  await t.throwsAsync(() => harness.invoke('echo', {}))
})
