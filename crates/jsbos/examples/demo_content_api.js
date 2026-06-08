#!/usr/bin/env node
/**
 * Content API Demo — All content passing methods
 *
 * Demonstrates all ways to pass content to AgentBuilder:
 * 1. Plain string (auto-wrapped to [{ type: 'text', text: ... }])
 * 2. Content.text()
 * 3. Content.parts([...]) with text + images
 * 4. agent.ask()
 * 5. agent.runSimple()
 * 6. agent.react()
 * 7. agent.stream()
 * 8. Direct JsContent array (no wrapper needed)
 *
 * Usage:
 *     node crates/jsbos/examples/demo_content_api.js
 */

import { BrainOS, Content, ContentPart } from '../index.js'

const CAT_URL = 'https://download.catpng.net/silver_tabby_cat_on_gray_pillow_beside_clear_glass_window-thumbnail.png'

async function demoPlainString() {
  console.log('='.repeat(60))
  console.log('Demo 1: Plain string (ask)')
  console.log('='.repeat(60))
  const brain = new BrainOS()
  await brain.start()
  const agent = brain.agent('assistant')
  const reply = await agent.ask('What is Python?')
  console.log(reply.substring(0, 200) + '...')
  await brain.stop()
}

async function demoContentText() {
  console.log('='.repeat(60))
  console.log('Demo 2: Content.text() (runSimple)')
  console.log('='.repeat(60))
  const brain = new BrainOS()
  await brain.start()
  const agent = brain.agent('assistant')
  const content = Content.text('What is 2 + 2?')
  const reply = await agent.runSimple(content)
  console.log(reply.substring(0, 200))
  await brain.stop()
}

async function demoContentParts() {
  console.log('='.repeat(60))
  console.log('Demo 3: Content.parts text+image (react)')
  console.log('='.repeat(60))
  const brain = new BrainOS()
  await brain.start()
  const agent = brain.agent('assistant')
  const content = Content.parts([
    ContentPart.text('Describe this image briefly.'),
    ContentPart.image(CAT_URL),
  ])
  const reply = await agent.react(content)
  console.log(reply.substring(0, 300) + '...')
  await brain.stop()
}

async function demoDirectJsContent() {
  console.log('='.repeat(60))
  console.log('Demo 4: Direct JsContent array (runSimple)')
  console.log('='.repeat(60))
  const brain = new BrainOS()
  await brain.start()
  const agent = brain.agent('assistant')
  // Pass JsContent array directly — no JSON serialization needed
  const content = [{ type: 'text', text: 'Say hello in one word.' }]
  const reply = await agent.runSimple(content)
  console.log(reply.substring(0, 200))
  await brain.stop()
}

async function demoStream() {
  console.log('='.repeat(60))
  console.log('Demo 5: Content.text() + stream')
  console.log('='.repeat(60))
  const brain = new BrainOS()
  await brain.start()
  const agent = brain.agent('assistant')
  const content = Content.text('Count from 1 to 5, one per line.')
  await agent.stream(content, (err, token) => {
    if (token.type === 'Text') process.stdout.write(token.text)
  })
  console.log('')
  await brain.stop()
}

async function main() {
  await demoPlainString()
  await demoContentText()
  await demoContentParts()
  await demoDirectJsContent()
  await demoStream()
  console.log('='.repeat(60))
  console.log('✅ All content API demos completed!')
  console.log('='.repeat(60))
}

main().catch(console.error).finally(() => process.exit(0))