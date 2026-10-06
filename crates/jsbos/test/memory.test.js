// Tests for the JavaScript memory store, including the shared Rust fixture.
import test from 'ava'
import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { dirname, join } from 'node:path'
import { Memory } from '../index.js'

const here = dirname(fileURLToPath(import.meta.url))
const FIXTURE = join(here, '..', '..', 'agent', 'tests', 'fixtures', 'memory_ranking.json')

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