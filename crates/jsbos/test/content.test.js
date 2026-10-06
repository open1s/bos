// Pins the multimodal content JSON shape, which the suite never exercised and
// the reference documented only in prose. crates/nbos/tests/test_content.py
// asserts the same shapes, so the two bindings cannot drift on what reaches the
// Rust backend.
import test from 'ava'
import { Binary, Content, ContentPart } from '../index.js'

test('Content.text serializes to a text object', (t) => {
  const content = Content.text('hello')
  t.deepEqual(content.toJSON(), { type: 'text', text: 'hello' })
  t.false(content.isMultimodal())
})

test('Content.image serializes to one image part with the jpeg content type', (t) => {
  t.deepEqual(Content.image('https://example.com/photo.jpg').toJSON(), [
    {
      type: 'binary',
      binary: {
        content_type: 'image/jpeg',
        source: { url: 'https://example.com/photo.jpg' },
      },
    },
  ])
})

test('Content.parts keeps order and marks the content multimodal', (t) => {
  const content = Content.parts([
    ContentPart.text('describe'),
    ContentPart.image('https://example.com/photo.jpg'),
  ])
  t.true(content.isMultimodal())
  const parts = content.toJSON()
  t.deepEqual(parts.map((p) => p.type), ['text', 'binary'])
  t.is(parts[1].binary.content_type, 'image/jpeg')
})

test('ContentPart.audio and audioUrl use an audio content type', (t) => {
  t.is(ContentPart.audio('QUJD', 'mp3').toJSON().binary.content_type, 'audio/mp3')
  t.deepEqual(ContentPart.audioUrl('https://example.com/a.mp3').toJSON().binary.source, {
    url: 'https://example.com/a.mp3',
  })
})

test('ContentPart.binary keeps base64 data as a base64 source', (t) => {
  t.deepEqual(ContentPart.binary('image/png', 'QUJD').toJSON().binary.source, {
    base64: 'QUJD',
  })
})

test('Binary.fromUrl returns the raw URL and fromBase64 a data URL', (t) => {
  t.is(Binary.fromUrl('image/png', 'https://example.com/a.png').url(), 'https://example.com/a.png')
  t.is(Binary.fromBase64('image/png', 'QUJD').url(), 'data:image/png;base64,QUJD')
})

test('Binary.contentType drives isImage and isAudio', (t) => {
  t.true(Binary.fromUrl('image/png', 'https://example.com/a.png').isImage())
  t.false(Binary.fromUrl('image/png', 'https://example.com/a.png').isAudio())
  t.true(Binary.fromUrl('audio/mpeg', 'https://example.com/a.mp3').isAudio())
})

test('Binary.toJSON omits an absent name', (t) => {
  t.deepEqual(Binary.fromUrl('image/png', 'https://example.com/a.png').toJSON(), {
    content_type: 'image/png',
    source: { url: 'https://example.com/a.png' },
  })
})