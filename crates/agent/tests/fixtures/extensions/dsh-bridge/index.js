module.exports = {
  apply(ctx, config) {
    ctx.tools.register({
      name: "dsh_bridge_echo",
      description: "Echo the arguments back through the DSH bridge.",
      parameters: {
        type: "object",
        properties: { value: { type: "string", description: "Anything." } },
        required: ["value"],
      },
      output: { schema: {}, render: () => null },
      execute(args) {
        return { echoed: args.value, plugin: config.name || "fixture" };
      },
    });
    ctx.commands.register({
      name: "bridge-hello",
      description: "Say hello from the bridged host entry.",
      handler: () => ({ kind: "success", text: "hello from the bridge" }),
    });
    ctx.logger.info("dsh-bridge fixture loaded");
  },
};
