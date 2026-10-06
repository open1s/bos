// Tests for the JavaScript memory store, including the shared Rust fixture.
import test from 'ava'
import { mkdtempSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { tmpdir } from 'node:os'
import { Memory, AgentBuilder } from '../index.js'

const here = dirname(fileURLToPath(import.meta.url))
const FIXTURE = join(here, '..', '..', 'agent', 'tests', 'fixtures', 'memory_ranking.json')
const LINES_FIXTURE = join(here, '..', '..', 'agent', 'tests', 'fixtures', 'memory_lines.jsonl')

test('add assigns identity', (t) => {
  const memory = new Memory()
  const item = memory.add('the sky is blue')
  t.is(item.content, 'the sky is blue')
  t.truthy(item.id)
  t.is(item.metadata, null)
  t.is(memory.len(), 1)
  t.false(memory.isEmpty())
})

test('search ranks by keyword overlap', (t) => {
  const memory = new Memory()
  memory.add('rust ownership and borrowing')
  memory.add('python asyncio event loop')
  memory.add('rust async runtimes')
  t.deepEqual(
    memory.search('rust async', 2).map((hit) => hit.content),
    ['rust async runtimes', 'rust ownership and borrowing'],
  )
})

test('empty query returns most recent first', (t) => {
  const memory = new Memory()
  memory.add('first')
  memory.add('second')
  t.deepEqual(memory.search('  ', 1).map((hit) => hit.content), ['second'])
})

test('remove and clear', (t) => {
  const memory = new Memory()
  const first = memory.add('a')
  memory.add('b')
  t.true(memory.remove(first.id))
  t.false(memory.remove(first.id))
  t.is(memory.len(), 1)
  memory.clear()
  t.true(memory.isEmpty())
})

test('toJSON round-trips', (t) => {
  const memory = new Memory()
  memory.add('x', { source: 'test' })
  const [item] = JSON.parse(JSON.stringify(memory))
  t.is(item.content, 'x')
  t.deepEqual(item.metadata, { source: 'test' })
})

test('matches the shared Rust fixture', (t) => {
  const data = JSON.parse(readFileSync(FIXTURE, 'utf8'))
  const memory = new Memory()
  for (const item of data.items) memory.add(item)
  t.deepEqual(
    memory.search(data.query, data.limit).map((hit) => hit.content),
    data.expected,
  )
})

test('ask injects recalled memory', async (t) => {
  const memory = new Memory()
  memory.add('the deploy key lives in 1password')
  const seen = []
  const builder = new AgentBuilder(null)
  builder.withMemory(memory)
  builder._inner = { runSimple: async (content) => { seen.push(content); return 'ok' } }
  await builder.ask('where is the deploy key')
  t.true(seen[0][0].text.includes('Relevant memory:'))
  t.true(seen[0][0].text.includes('the deploy key lives in 1password'))
})

test('ask without memory is untouched', async (t) => {
  const seen = []
  const builder = new AgentBuilder(null)
  builder._inner = { runSimple: async (content) => { seen.push(content); return 'ok' } }
  await builder.ask('hello')
  t.is(seen[0][0].text, 'hello')
})

test('withMemory is chainable', (t) => {
  const builder = new AgentBuilder(null)
  const memory = new Memory()
  t.is(builder.withMemory(memory), builder)
  t.is(builder._memory, memory)
})

test('save and load round-trip', (t) => {
  const dir = mkdtempSync(join(tmpdir(), 'bos-memory-'))
  const path = join(dir, 'memory.jsonl')
  const memory = new Memory()
  memory.add('first', { source: 'test' })
  memory.add('second')
  t.is(memory.save(path), 2)
  const loaded = Memory.load(path)
  t.deepEqual(loaded.all().map((i) => i.content), ['first', 'second'])
  t.deepEqual(loaded.all()[0].metadata, { source: 'test' })
})

test('loads the shared JSON-lines fixture', (t) => {
  const loaded = Memory.load(LINES_FIXTURE)
  t.deepEqual(loaded.all().map((i) => i.content), [
    'rust ownership and borrowing',
    'the staging deploy needs VPN',
  ])
  t.deepEqual(loaded.all()[1].metadata, { source: 'runbook' })
})

test('recallLimit bounds injection', async (t) => {
  const memory = new Memory()
  memory.add('rust async runtimes')
  memory.add('rust ownership and borrowing')
  const seen = []
  const builder = new AgentBuilder(null)
  builder.withMemory(memory, 1)
  builder._inner = { runSimple: async (content) => { seen.push(content); return 'ok' } }
  await builder.ask('rust')
  t.is((seen[0][0].text.match(/\n- /g) || []).length, 1)
})