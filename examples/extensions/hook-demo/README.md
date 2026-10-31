# Hook Demo

A process extension that demonstrates every host hook point and the contribution
kinds a BSH package can carry.

## Hook points

It subscribes to:

- app and workspace: `app.started`, `workspace.opened`
- session log: `session.created`, `session.updated`, `session.deleted`
- trajectory: `trajectory.recorded`
- agent turn: `agent.started`, `agent.completed`, `agent.failed`
- skills: `skill.loaded`
- instructions: `instruction.build`
- model: `model.requested`, `model.completed`
- MCP: `mcp.connected`
- tools: `tool.invoked`, `tool.completed`, `tool.failed`
- network: `network.request`
- audio: `audio.captured`
- extension lifecycle: `extension.loaded`

The server records each `events/event` notification in memory. Call the
`hook-log` tool to see what arrived:

~~~text
bsh ext mock examples/extensions/hook-demo hook-log '{}'
~~~

The package asks for process, skill, model, agent and workflow capabilities, so
a local install needs explicit grants before it can activate:

~~~text
bsh ext install examples/extensions/hook-demo
for cap in process.execute agent.skills.provide agent.models.provide \
           agent.agents.provide agent.workflows.provide; do
  bsh ext grant com.example.hook-demo "$cap" --scope any
done
bsh ext info com.example.hook-demo   # state: active, applied: true
bsh ext doctor
~~~

## Other contributions

The same package carries a command, prompt, scoped context, panel, theme,
preset, model provider, agent role, workflow and config schema, so it exercises
the provider and UI surfaces alongside the event hooks. The workflow's single
step calls this package's own `hook-log` tool.
