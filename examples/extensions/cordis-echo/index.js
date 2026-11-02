// A real Cordis host entry. BSH executes this through the sandboxed Node
// bridge; it never gets in-process access to the host.
module.exports = {
  apply(ctx, config) {
    ctx.tools.register({
      name: "echo",
      description: "Echo a value through the Cordis runtime.",
      parameters: {
        type: "object",
        properties: { value: { type: "string" } },
        required: ["value"],
      },
      output: { schema: {}, render: () => null },
      execute(args) {
        return {
          echoed: args.value,
          source: "cordis",
          config: config && config.name ? config.name : "default",
        };
      },
    });
    ctx.commands.register({
      name: "hello",
      description: "Say hello from the Cordis runtime.",
      handler: () => ({ kind: "success", text: "hello from cordis" }),
    });
    ctx.skills.register({
      name: "greet",
      description: "Greet the reader from the Cordis runtime.",
      content: "# Greet\n\nSay hello from the Cordis runtime.",
    });
  },
};
