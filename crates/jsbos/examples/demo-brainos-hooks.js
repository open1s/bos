import { BrainOS, HookEvent } from '../index.js'

async function demo() {
  console.log('=== JSBOS BrainOS Hook Demo ===\n')

  const brain = new BrainOS({
    apiKey: process.env.OPENAI_API_KEY || 'nvapi-...',
  })

  await brain.start()

  console.log('1. Building agent with hooks via .hook()...')

  const agent = await brain.agent('assistant')
    .hook(HookEvent.BeforeToolCall, (ctx) => {
      console.log('   [BeforeToolCall]', ctx?.data?.tool_name || 'unknown')
      return 'continue'
    })
    .hook(HookEvent.AfterToolCall, (ctx) => {
      console.log('   [AfterToolCall]', ctx?.data?.tool_name || 'unknown')
      return 'continue'
    })
    .hook(HookEvent.BeforeLlmCall, (ctx) => {
      console.log('   [BeforeLlmCall] Starting LLM call')
      return 'continue'
    })
    .hook(HookEvent.AfterLlmCall, (ctx) => {
      console.log('   [AfterLlmCall] LLM call completed')
      return 'continue'
    })
    .hook(HookEvent.OnError, (ctx) => {
      console.log('   [OnError]', ctx?.data?.error || 'unknown error')
      return 'continue'
    })
    .start()

  console.log('   Hooks registered!\n')

  console.log('2. Hook decisions:')
  console.log('   - "continue" or return nothing: proceed normally')
  console.log('   - "abort": abort the current operation')
  console.log('   - "error:message": return an error\n')

  console.log('3. Running agent (hooks will fire during execution)...')
  try {
    const result = await agent.ask('Say "test" in one word')
    console.log('   Result:', result?.substring?.(0, 100) + '...' || result)
  } catch (e) {
    console.log('   Error (may be expected with rate limiting):', e.message?.substring(0, 100))
  }

  console.log('\n=== Done ===')
}

demo().catch(console.error)
