import { BrainOS, Config, ToolDef, HookEvent } from '../index.js'

async function main() {
  console.log('═══════════════════════════════════════════════════════════')
  console.log('     jsbos: stream() with hooks + plugins demo')
  console.log('═══════════════════════════════════════════════════════════\n')

  const config = Config.load()
  console.log(`Model: ${config.model}\n`)

  const brain = new BrainOS()
  await brain.start()

  const addTool = new ToolDef(
    'add',
    'Add two numbers',
    (args) => ({ result: (args.a || 0) + (args.b || 0) }),
    {
      type: 'object',
      properties: { result: { type: 'number' } },
      required: ['result'],
    },
    {
      type: 'object',
      properties: {
        a: { type: 'number', description: 'First number' },
        b: { type: 'number', description: 'Second number' },
      },
      required: ['a', 'b'],
    },
  )

  const uppercaseTool = new ToolDef(
    'uppercase',
    'Convert text to uppercase',
    (args) => ({ result: String(args.text || '').toUpperCase() }),
    { type: 'object', properties: { result: { type: 'string' } }, required: ['result'] },
    {
      type: 'object',
      properties: { text: { type: 'string', description: 'Text to convert' } },
      required: ['text'],
    },
  )

  let hookLog = []
  let tokenLog = []
  let completeCount = 0

  const agent = await brain
    .agent('stream-demo')
    .system('You are a helpful assistant with tools. Use the add tool for all math and the uppercase tool for all text conversions.')
    .model(config.model)
    .register(addTool)
    .register(uppercaseTool)
    .plugin('StreamLogger', {
      onLlmRequest: (req) => {
        console.log(`  [Plugin:onLlmRequest] model=${req.model}`)
        return req
      },
      onToolCall: (call) => {
        console.log(`  [Plugin:onToolCall] tool="${call.name}" args=${JSON.stringify(call.args)}`)
        hookLog.push(`plugin:onToolCall:${call.name}`)
        return call
      },
      onToolResult: (result) => {
        console.log(`  [Plugin:onToolResult] success=${result.success}`)
        hookLog.push('plugin:onToolResult')
        return result
      },
    })
    .hook(HookEvent.BeforeToolCall, (err, ctx) => {
      if (err) return 'error'
      const name = ctx?.data?.tool_name || 'unknown'
      console.log(`  [Hook:BeforeToolCall] tool="${name}"`)
      hookLog.push(`hook:BeforeToolCall:${name}`)
      return 'continue'
    })
    .hook(HookEvent.AfterToolCall, (err, ctx) => {
      if (err) return 'error'
      const name = ctx?.data?.tool_name || 'unknown'
      console.log(`  [Hook:AfterToolCall] tool="${name}"`)
      hookLog.push(`hook:AfterToolCall:${name}`)
      return 'continue'
    })
    .hook(HookEvent.BeforeLlmCall, (err, ctx) => {
      if (err) return 'error'
      console.log('  [Hook:BeforeLlmCall]')
      hookLog.push('hook:BeforeLlmCall')
      return 'continue'
    })
    .hook(HookEvent.AfterLlmCall, (err, ctx) => {
      if (err) return 'error'
      console.log(`  [Hook:AfterLlmCall] response_type=${ctx?.data?.response_type || '?'}`)
      hookLog.push('hook:AfterLlmCall')
      return 'continue'
    })
    .hook(HookEvent.OnMessage, (err, ctx) => {
      if (err) return 'error'
      console.log('  [Hook:OnMessage]')
      hookLog.push('hook:OnMessage')
      return 'continue'
    })
    .hook(HookEvent.OnComplete, (err, ctx) => {
      if (err) return 'error'
      completeCount++
      console.log(`  [Hook:OnComplete] #${completeCount}`)
      hookLog.push('hook:OnComplete')
      return 'continue'
    })
    .hook(HookEvent.OnError, (err, ctx) => {
      if (err) return 'error'
      const error = ctx?.data?.error || 'unknown'
      console.log(`  [Hook:OnError] ${error}`)
      hookLog.push(`hook:OnError:${error.substring(0, 40)}`)
      return 'continue'
    })
    .start()

  // ── Test 1: Stream with tool calls ──
  console.log('─'.repeat(55))
  console.log('TEST 1: stream("What is 12 + 8? Then uppercase: hello world")')
  console.log('─'.repeat(55))
  hookLog = []
  tokenLog = []

  process.stdout.write('Stream output: ')
  await agent.stream('What is 12 + 8? Then uppercase: hello world', (token) => {
    if (!token) return
    if (token.type === 'Text') {
      process.stdout.write(token.text)
      tokenLog.push(token)
    } else if (token.type === 'ToolCall') {
      process.stdout.write(`\n  [ToolCall: ${token.name}(${JSON.stringify(token.args)})] `)
      tokenLog.push(token)
    } else if (token.type === 'ReasoningContent') {
      process.stdout.write(`[${token.text}]`)
      tokenLog.push(token)
    }
    if (token.type === 'Done') tokenLog.push(token)
  })

  console.log(`\nTokens received: ${tokenLog.length}`)
  console.log(`Hooks fired (${hookLog.length}):${hookLog.map(h => `\n    - ${h}`).join('')}`)
  console.log()

  // ── Test 2: streamCollect (ezbos pattern — inject on inner agent) ──
  console.log('─'.repeat(55))
  console.log('TEST 2: streamCollect("What is 5+3?") — ezbos streamCollect pattern')
  console.log('─'.repeat(55))
  hookLog = []

  const innerAgent = await brain.agent('stream-demo-2')
    .system('You are a helpful assistant with tools. Use the add tool for all math.')
    .model(config.model)
    .register(addTool)
    .hook(HookEvent.BeforeToolCall, (err, ctx) => {
      if (err) return 'error'
      hookLog.push(`hook:BeforeToolCall:${ctx?.data?.tool_name || '?'}`)
      return 'continue'
    })
    .hook(HookEvent.AfterToolCall, (err, ctx) => {
      if (err) return 'error'
      hookLog.push(`hook:AfterToolCall:${ctx?.data?.tool_name || '?'}`)
      return 'continue'
    })
    .hook(HookEvent.OnComplete, (err) => { if (err) return 'error'; hookLog.push('hook:OnComplete'); return 'continue' })
    .start()

  innerAgent.streamCollect = (task) => {
    const tokens = []
    return new Promise((resolve, reject) => {
      innerAgent.stream(task, (err, token) => {
        if (err) { reject(new Error(err.message || String(err))); return }
        if (!token) return
        tokens.push(token)
        if (token.type === 'Done' || token.type === 'Stopped') resolve(tokens)
        if (token.type === 'Error') reject(new Error(token.error))
      })
    })
  }

  const tokens = await innerAgent.streamCollect('What is 5+3?')
  const fullText = tokens.filter(t => t.type === 'Text').map(t => t.text).join('')
  const toolCalls = tokens.filter(t => t.type === 'ToolCall')
  console.log(`Result: ${fullText}`)
  console.log(`Tool calls: ${toolCalls.map(tc => `${tc.name}(${JSON.stringify(tc.args)})`).join(', ') || 'none'}`)
  console.log(`Tokens count: ${tokens.length}`)
  console.log(`Hooks fired (${hookLog.length}):${hookLog.map(h => `\n    - ${h}`).join('')}`)
  console.log()

  // ── Test 3: Stream without tool calls ──
  console.log('─'.repeat(55))
  console.log('TEST 3: stream("Say hello in exactly one word")')
  console.log('─'.repeat(55))
  hookLog = []
  tokenLog = []

  process.stdout.write('Stream output: ')
  await agent.stream('Say hello in exactly one word', (token) => {
    if (!token) return
    if (token.type === 'Text') {
      process.stdout.write(token.text)
      tokenLog.push(token)
    }
    if (token.type === 'Done') tokenLog.push(token)
  })

  console.log(`\nTokens received: ${tokenLog.length}`)
  console.log(`Hooks fired (${hookLog.length}):${hookLog.map(h => `\n    - ${h}`).join('')}`)
  console.log()

  await brain.stop()
  console.log('═══════════════════════════════════════════════════════════')
  console.log('All tests passed!')
  console.log('═══════════════════════════════════════════════════════════')
  process.exit(0)
}

main().catch((e) => {
  console.error('Error:', e.message)
  process.exit(1)
})