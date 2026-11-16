// Tests for the agent-runtime controls that arrived with builtin tools 2.0:
// plans, goals, steering, deferral, live skills and background jobs. No LLM
// call is made; every exercise is local to the native agent state.
import test from 'ava'
import {
  AgentBuilder,
  Goal,
  GoalStore,
  JobRegistry,
  ToolDef,
  renderTemplate,
  withMemory,
  estimateTokens,
} from '../index.js'

const CONFIG = {
  name: 'agentic-test',
  model: 'openai/gpt-4o',
  baseUrl: 'http://127.0.0.1:9/v1',
  apiKey: 'test',
  systemPrompt: 'test',
  temperature: 0,
  timeoutSecs: 5,
  apiMode: 'chat',
}

async function started(extra = {}) {
  return new AgentBuilder(null, { ...CONFIG, ...extra }).start()
}

test('plan snapshot round-trips and the plan tool registers once', async (t) => {
  const agent = await started()
  t.deepEqual(agent.planItems(), [])

  agent.setPlan([
    { text: 'write tests', status: 'in_progress' },
    { text: 'ship', status: 'pending' },
  ])
  t.deepEqual(agent.planItems(), [
    { text: 'write tests', status: 'in_progress' },
    { text: 'ship', status: 'pending' },
  ])

  agent.clearPlan()
  t.deepEqual(agent.planItems(), [])

  t.true(agent.addPlanTool())
  t.false(agent.addPlanTool())
  t.true(agent.toolNames.includes('update_plan'))
})

test('steering queues and returns undelivered tickets', async (t) => {
  const agent = await started()
  t.true(agent.steer('focus on the parser', 7))
  t.true(agent.steer('then the lexer', 8))
  t.is(agent.steerDepth(), 2)
  t.deepEqual(agent.takeUndeliveredSteers(), [7, 8])
  t.is(agent.steerDepth(), 0)
})

test('deferral withholds tools by name and prefix', async (t) => {
  const agent = await started()
  t.false(agent.hasDeferredTools())
  agent.deferTool('secret')
  agent.deferToolPrefix('mcp_')
  t.true(agent.isDeferredTool('secret'))
  t.true(agent.isDeferredTool('mcp_search'))
  t.false(agent.isDeferredTool('other'))
  t.true(agent.hasDeferredTools())
})

test('inline skills register, list and remove', async (t) => {
  const agent = await started()
  agent.registerSkill('reviewer', 'Review code', 'Read the diff first.')
  const skills = agent.listSkills()
  const reviewer = skills.find((skill) => skill.name === 'reviewer')
  t.truthy(reviewer)
  t.is(reviewer.description, 'Review code')
  t.is(reviewer.instructions, 'Read the diff first.')

  t.is(agent.removeSkillsByNames(['reviewer']), 1)
  t.false(agent.listSkills().some((skill) => skill.name === 'reviewer'))
})

test('live prompt sections can be added and removed', async (t) => {
  const agent = await started()
  agent.addLivePromptSection('Stay terse.')
  t.true(agent.removeLivePromptSection('Stay terse.'))
  t.false(agent.removeLivePromptSection('Stay terse.'))
})

test('config carries the new behaviour knobs', async (t) => {
  const agent = await started({
    personaComplete: true,
    compactionInputTokens: 9000,
    toolOutputCap: 4000,
    parallelToolCalls: true,
    webSearch: false,
    maxSteps: 4,
  })
  const config = agent.config
  t.true(config.persona_complete)
  t.is(config.compaction_input_tokens, 9000)
  t.is(config.tool_output_cap, 4000)
  t.true(config.parallel_tool_calls)
  t.is(config.max_steps, 4)
})

test('context budget and skill catalogue update before and after start', async (t) => {
  const builder = new AgentBuilder(null, { ...CONFIG })
  t.is(builder.contextBudget(), 0)
  builder.contextBudget(2048)
  t.is(builder.contextBudget(), 2048)
  builder.skillCatalogue(true)

  const agent = await builder.start()
  t.is(agent.contextBudget(), 2048)
  t.true(agent.skillCatalogue())
  agent.setContextBudget(4096)
  t.is(agent.contextBudget(), 4096)
  agent.setSkillCatalogue(false)
  t.false(agent.skillCatalogue())
})

test('goals transition and settle through their store', (t) => {
  const goal = Goal.create('Ship the release')
  t.is(goal.objective, 'Ship the release')
  t.is(goal.state, 'active')
  t.is(goal.activation, 'armed')
  t.true(goal.advance())
  t.is(goal.rounds, 1)
  t.true(goal.pause())
  t.is(goal.state, 'paused')
  t.true(goal.resume())
  t.true(goal.complete())
  t.is(goal.state, 'completed')
  t.true(goal.toJSON().state === 'completed')

  const restored = Goal.fromJSON(JSON.stringify(goal.toJSON()))
  t.is(restored.objective, 'Ship the release')
  t.is(restored.state, 'completed')

  const active = Goal.create('Fix the bug')
  const store = new GoalStore(active)
  t.true(store.complete())
  t.true(store.hasSettle())
  t.deepEqual(store.takeSettle(), { type: 'complete' })
  t.false(store.hasSettle())
  t.is(store.snapshot().state, 'completed')
})

test('goal tools register against a store once', async (t) => {
  const agent = await started()
  const store = new GoalStore(Goal.create('Ship'))
  t.is(agent.addGoalTools(store), 2)
  t.is(agent.addGoalTools(store), 0)
  t.true(agent.toolNames.includes('get_goal'))
  t.true(agent.toolNames.includes('update_goal'))
})

test('job tools register against a roster once', async (t) => {
  const agent = await started()
  const jobs = new JobRegistry()
  t.is(agent.addJobTools(jobs, 'default'), 2)
  t.is(agent.addJobTools(jobs, 'default'), 0)
  t.true(agent.toolNames.includes('job_output'))
  t.true(agent.toolNames.includes('job_kill'))
})

test('the bash tool can route background jobs', async (t) => {
  const agent = await started()
  const jobs = new JobRegistry()
  await agent.addBashTool('bash', null, jobs, 'default')
  t.true(agent.toolNames.includes('bash'))
})

test('job roster is empty and bounded', async (t) => {
  const jobs = new JobRegistry(10, 1024)
  t.deepEqual(jobs.list('default'), [])
  t.is(jobs.runningCount('default'), 0)
  t.is(jobs.settledCount('default'), 0)
  t.is(jobs.clearSettled('default'), 0)
  t.is(jobs.get('missing'), null)
  t.is(jobs.output('missing'), null)
  t.false(jobs.kill('missing'))
  t.is(typeof jobs.version(), 'number')
})

test('prompt-template helpers mirror the Rust template module', (t) => {
  t.is(renderTemplate('Hello {{ name }}!', { name: 'BOS' }), 'Hello BOS!')
  t.throws(() => renderTemplate('Hello {{name}}!', {}))
  t.is(withMemory('You are {{role}}.\n\n{{memory}}', 'Remember: tea.'), 'You are {{role}}.\n\nRemember: tea.')
  t.is(withMemory('base', 'Remember: tea.'), 'base\n\nRemember: tea.')
  t.true(estimateTokens('one two three') > 0)
})

test('skill metadata reports version and tags', async (t) => {
  const agent = await started()
  agent.registerSkill('reviewer', 'Review code', 'Read the diff first.')
  const reviewer = agent.listSkills().find((skill) => skill.name === 'reviewer')
  t.is(reviewer.version, '1.0.0')
  t.deepEqual(reviewer.tags, [])
})

test('live skills are visible without restarting the agent', async (t) => {
  const agent = await started()
  agent.addLiveSkill('live', 'Live skill', 'Act now.')
  t.true(agent.liveSkills().some((skill) => skill.name === 'live'))
  t.true(agent.listSkills().some((skill) => skill.name === 'live'))

  t.is(agent.removeLiveSkillsByPaths(['.bos/skills/live']), 1)
  t.false(agent.liveSkills().some((skill) => skill.name === 'live'))
})

test('live tools appear in full definitions with metadata', async (t) => {
  const agent = await started()
  const tool = new ToolDef(
    'lookup',
    'Look up a record',
    (args) => ({ ok: true }),
    {},
    { type: 'object', properties: {} },
    null,
    { risk: 'low', side_effects: ['read'], permissions: ['read'] },
  )
  t.is(await agent.addLiveTool(tool), agent)
  t.true(agent.toolNames.includes('lookup'))

  const lookup = agent.toolDefinitions().find((item) => item.name === 'lookup')
  t.is(lookup.description, 'Look up a record')
  t.is(lookup.metadata.risk, 'low')
  t.deepEqual(lookup.metadata.permissions, ['read'])

  t.is(agent.removeLiveTools(['lookup']), 1)
  t.false(agent.toolNames.includes('lookup'))
})

test('compactNow on an empty session returns null', async (t) => {
  const agent = await started()
  t.is(await agent.compactNow(0), null)
})

test('close releases runtime extensions', async (t) => {
  const agent = await started()
  agent.addPlanTool()
  t.true(agent.toolNames.includes('update_plan'))
  agent.close()
  t.deepEqual(agent.toolNames, [])
})
