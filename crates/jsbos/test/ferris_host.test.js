// Full Ferris host: package store, activation and invocation.
import test from 'ava'
import { chmodSync, mkdtempSync, mkdirSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { FerrisHost } from '../index.js'

const HERE = dirname(fileURLToPath(import.meta.url))
const EXAMPLES = join(HERE, '..', '..', '..', 'examples', 'extensions')

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
kind = "declarative"

[capabilities]
required = ["agent.tools.register"]

[[contribution]]
kind = "tool"
id = "echo"
`

function makePackage() {
  const root = mkdtempSync(join(tmpdir(), 'ferris-pkg-'))
  writeFileSync(join(root, 'extension.toml'), MANIFEST)
  mkdirSync(join(root, 'tools'))
  writeFileSync(
    join(root, 'tools', 'echo.json'),
    JSON.stringify({ description: 'Echo', implementation: { kind: 'echo_arguments' } }),
  )
  return root
}

const PROCESS_MANIFEST = MANIFEST
  .replace('com.example.echo', 'com.example.process')
  .replace('name = "Echo"', 'name = "Process"')
  .replace('description = "Echoes its input"', 'description = "Process echo"')
  .replace('kind = "declarative"', 'kind = "process"\n[runtime.config]\nentry = "server.py"')
  .replace('required = ["agent.tools.register"]', 'required = ["process.execute"]')

const PROCESS_SERVER = `#!/usr/bin/env python3
import json
import sys

for line in sys.stdin:
    request = json.loads(line)
    method = request.get("method")
    if method == "initialize":
        result = {
            "protocolVersion": "1.0",
            "token": request.get("params", {}).get("token"),
        }
    elif method == "tools/call":
        result = request.get("params", {}).get("arguments")
    else:
        result = {}
    print(json.dumps({"jsonrpc": "2.0", "id": request.get("id"), "result": result}), flush=True)
`

function makeProcessPackage() {
  const root = mkdtempSync(join(tmpdir(), 'ferris-proc-'))
  writeFileSync(join(root, 'extension.toml'), PROCESS_MANIFEST)
  mkdirSync(join(root, 'tools'))
  writeFileSync(join(root, 'tools', 'echo.json'), JSON.stringify({ description: 'Echo' }))
  const server = join(root, 'server.py')
  writeFileSync(server, PROCESS_SERVER)
  chmodSync(server, 0o755)
  return root
}

function makeHost() {
  return new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')))
}

test('install list plan and package metadata', (t) => {
  const host = makeHost()
  t.deepEqual(host.install(makePackage()), { id: 'com.example.echo', version: '1.0.0' })
  t.deepEqual(host.installed(), [{ id: 'com.example.echo', version: '1.0.0' }])
  t.deepEqual(host.versions('com.example.echo'), ['1.0.0'])
  t.is(host.activeVersion('com.example.echo'), '1.0.0')
  t.true(host.packageRoot('com.example.echo', '1.0.0').includes('com.example.echo'))
  t.deepEqual(host.plan(), ['com.example.echo'])
})

test('process runtime starts and invokes', async (t) => {
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    policyMode: 'advisory',
    processSandbox: false,
  })
  host.install(makeProcessPackage())
  const contributions = await host.apply()
  t.is(contributions['com.example.process'][0].id, 'echo')
  t.deepEqual(await host.invoke('com.example.process', 'tool', 'echo', { ok: true }), {
    ok: true,
  })
  await host.unload()
})

test('extism runtime starts and invokes', async (t) => {
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    policyMode: 'advisory',
  })
  host.install(join(EXAMPLES, 'extism-echo'))
  const contributions = await host.apply()
  t.is(contributions['com.example.extism-echo'][0].id, 'echo')
  const result = await host.invoke('com.example.extism-echo', 'tool', 'echo', { x: 1 })
  t.truthy(result)
  await host.unload()
})

test('mcp runtime starts and invokes', async (t) => {
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    policyMode: 'advisory',
    processSandbox: false,
  })
  host.install(join(EXAMPLES, 'mcp-echo'))
  const contributions = await host.apply()
  t.truthy(contributions['com.example.mcp-echo'].find((entry) => entry.id === 'echo'))
  const result = await host.invoke('com.example.mcp-echo', 'tool', 'echo', { x: 1 })
  t.truthy(result)
  await host.unload()
})

test('cordis runtime starts and invokes', async (t) => {
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    policyMode: 'advisory',
    processSandbox: false,
  })
  host.install(join(EXAMPLES, 'cordis-echo'))
  const contributions = await host.apply()
  t.is(contributions['com.example.cordis-echo'][0].id, 'echo')
  const result = await host.invoke('com.example.cordis-echo', 'tool', 'echo', { value: 'x' })
  t.is(result.echoed, 'x')
  await host.unload()
})

test('wasm component runtime starts and invokes', async (t) => {
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    policyMode: 'advisory',
  })
  host.install(join(EXAMPLES, 'wasm-echo'))
  const contributions = await host.apply()
  t.is(contributions['com.example.wasm-echo'][0].id, 'echo')
  t.deepEqual(await host.invoke('com.example.wasm-echo', 'tool', 'echo', { x: 1 }), {
    x: 1,
  })
  await host.unload()
})

test('contribution callbacks fire on apply and unload', async (t) => {
  const applied = []
  const revoked = []
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    onContributions: (payload) => applied.push(payload),
    onRevoke: (payload) => revoked.push(payload),
  })
  host.install(makePackage())
  await host.apply()
  t.is(applied.length, 1)
  t.is(applied[0].extension, 'com.example.echo')
  t.is(applied[0].contributions[0].id, 'echo')

  await host.unload()
  t.is(revoked.length, 1)
  t.is(revoked[0].extension, 'com.example.echo')
})

test('event subscriptions receive published events', async (t) => {
  const events = []
  const host = makeHost()
  const id = host.subscribeEvents(['demo.*'], (event) => events.push(event))
  t.true(typeof id === 'number')

  const report = host.publishEvent('demo.ping', { n: 1 })
  t.is(report.delivered, 1)

  for (let attempt = 0; attempt < 50 && events.length === 0; attempt++) {
    await new Promise((resolve) => setTimeout(resolve, 10))
  }
  t.is(events.length, 1)
  t.is(events[0].event_type, 'demo.ping')
  t.deepEqual(events[0].payload, { n: 1 })

  t.true(host.unsubscribeEvents(id))
  t.false(host.unsubscribeEvents(id))
})

test('apply contributes and invokes', async (t) => {
  const host = makeHost()
  host.install(makePackage())

  const contributions = await host.apply()
  t.is(contributions['com.example.echo'][0].kind, 'tool')
  t.is(contributions['com.example.echo'][0].id, 'echo')

  t.deepEqual(await host.invoke('com.example.echo', 'tool', 'echo', { x: 1 }), { x: 1 })
  t.is(host.contributions()['com.example.echo'][0].id, 'echo')

  await host.unload()
  t.deepEqual(host.contributions(), {})
  await t.throwsAsync(() => host.invoke('com.example.echo', 'tool', 'echo', {}))
})

test('a rejecting contribution callback leaves no phantom entry', async (t) => {
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    onContributions: () => {
      throw new Error('boom')
    },
  })
  host.install(makePackage())
  await t.throwsAsync(() => host.apply())
  t.deepEqual(host.contributions(), {})
})

test('replacing a linked extension shuts the old one down', async (t) => {
  let revoked = 0
  let shutDown = 0
  const host = new FerrisHost(mkdtempSync(join(tmpdir(), 'ferris-host-')), {
    onRevoke: () => {
      revoked += 1
    },
  })
  const start = async () => [{ kind: 'tool', id: 'echo', payload: {} }]
  const invoke = async (call) => call.input
  const shutdown = async () => {
    shutDown += 1
  }
  await host.registerExtension(
    MANIFEST,
    mkdtempSync(join(tmpdir(), 'ferris-linked-')),
    start,
    invoke,
    shutdown,
  )
  await host.registerExtension(
    MANIFEST,
    mkdtempSync(join(tmpdir(), 'ferris-linked-')),
    start,
    invoke,
    shutdown,
  )
  t.is(revoked, 1)
  t.is(shutDown, 1)
  await host.unload()
  t.is(revoked, 2)
  t.is(shutDown, 2)
})

test('host registers applied tools on an agent registry', async (t) => {
  const host = makeHost()
  host.install(makePackage())
  await host.apply()
  const registry = {
    tools: [],
    add(td) {
      this.tools.push(td)
    },
  }
  const registered = await host.registerAgentTools({ _tools: registry })
  t.deepEqual(registered, [{ extension: 'com.example.echo', name: 'echo' }])
  t.is(registry.tools[0].name, 'echo')
  t.is(typeof registry.tools[0].callback, 'function')
  await host.unload()
})

test('a linked JavaScript extension registers and invokes', async (t) => {
  const host = makeHost()
  const contributions = await host.registerExtension(
    MANIFEST,
    mkdtempSync(join(tmpdir(), 'ferris-linked-')),
    async () => [{ kind: 'tool', id: 'echo', payload: { description: 'Echo' } }],
    async (call) => call.input,
  )
  t.is(contributions[0].id, 'echo')
  t.is(host.contributions()['com.example.echo'][0].id, 'echo')
  t.deepEqual(await host.invoke('com.example.echo', 'tool', 'echo', { ok: true }), {
    ok: true,
  })
  await host.unload()
  t.deepEqual(host.contributions(), {})
})

test('enable disable and uninstall', (t) => {
  const host = makeHost()
  host.install(makePackage())
  host.setEnabled('com.example.echo', false)
  t.deepEqual(host.plan(), [])
  host.setEnabled('com.example.echo', true)
  t.deepEqual(host.plan(), ['com.example.echo'])
  host.uninstall('com.example.echo', false)
  t.deepEqual(host.installed(), [])
})

test('an unknown grant scope is refused', (t) => {
  const host = makeHost()
  host.install(makePackage())
  t.throws(() => host.grant('com.example.echo', 'agent.tools.register', 'typo-scope'))
  t.false('agent.tools.register' in host.grants('com.example.echo'))
})

test('grants are persisted and revocable', (t) => {
  const host = makeHost()
  host.install(makePackage())
  host.grant('com.example.echo', 'agent.tools.register', 'any', 'test')
  t.truthy(host.grants('com.example.echo')['agent.tools.register'])
  t.true(host.revokeGrant('com.example.echo', 'agent.tools.register'))
  t.deepEqual(host.grants('com.example.echo'), {})
  t.is(host.auditDegraded(), null)
})
