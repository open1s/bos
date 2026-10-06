// addRemoteAgentTool is native-only in both bindings (neither high-level
// wrapper exposes it), so this drives the native Agent directly.
import test from 'ava'
import { Agent, Bus } from '../index.js'

test.serial('addRemoteAgentTool registers a remote tool over the bus', async (t) => {
  const bus = await Bus.create()
  t.teardown(async () => {
    try {
      await bus.close()
    } catch {
      // Already closed by the failure path.
    }
  })

  const agent = await Agent.createWithBus(
    {
      name: 'remote-test',
      model: 'test',
      baseUrl: 'http://127.0.0.1:1/v1',
      apiKey: 'test-key',
      systemPrompt: 'You are a test.',
      temperature: 0.0,
      timeoutSecs: 5,
    },
    await bus.session(),
  )
  t.teardown(() => agent.close())

  t.false(agent.listTools().includes('remoteEcho'))
  await agent.addRemoteAgentTool('remoteEcho', 'zenoh/remote_echo')
  t.true(agent.listTools().includes('remoteEcho'))
})