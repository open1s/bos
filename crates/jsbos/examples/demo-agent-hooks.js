import { Agent, HookEvent } from '../index.js'

async function demo() {
  console.log('=== JSBOS Agent Hook Demo ===\n')

  console.log('1. Creating agent...')
  const config = {
    name: 'assistant',
    model: 'gpt-4',
    apiKey: process.env.OPENAI_API_KEY || 'sk-test',
    baseUrl: 'https://api.openai.com/v1',
    systemPrompt: 'You are a helpful assistant.',
    temperature: 0.7,
    timeoutSecs: 120,
  }

  const agent = await Agent.create(config)
  console.log('   Agent created!\n')

  console.log('2. Registering hooks...')

  agent.registerHook(HookEvent.BeforeToolCall, (ctx) => {
    console.log('   [BeforeToolCall]', ctx.data.tool_name || 'unknown')
    return 'continue'
  })

  agent.registerHook(HookEvent.AfterToolCall, (ctx) => {
    console.log('   [AfterToolCall]', ctx.data.tool_name || 'unknown')
    return 'continue'
  })

  agent.registerHook(HookEvent.BeforeLlmCall, (ctx) => {
    console.log('   [BeforeLlmCall] Starting LLM call')
    return 'continue'
  })

  agent.registerHook(HookEvent.AfterLlmCall, (ctx) => {
    console.log('   [AfterLlmCall] LLM call completed')
    return 'continue'
  })

  agent.registerHook(HookEvent.OnError, (ctx) => {
    console.log('   [OnError]', ctx.data.error || 'unknown error')
    return 'continue'
  })

  console.log('   Hooks registered!\n')

  console.log('3. Available hook events:')
  console.log('   - BeforeToolCall / AfterToolCall: around tool execution')
  console.log('   - BeforeLlmCall / AfterLlmCall: around LLM calls')
  console.log('   - OnMessage / OnComplete: message and completion events')
  console.log('   - OnError: when errors occur\n')

  console.log('4. Hook decisions:')
  console.log('   - "continue" or return nothing: proceed normally')
  console.log('   - "abort": abort the current operation')
  console.log('   - "error:message": return an error\n')

  console.log('5. When agent.run_simple() or agent.react() is called,')
  console.log('   hooks will fire at the appropriate times.\n')

  agent.close()

  console.log('=== Done ===')
}

demo()
  .then(() => process.exit(0))
  .catch((err) => {
    console.error(err)
    process.exit(1)
  })
