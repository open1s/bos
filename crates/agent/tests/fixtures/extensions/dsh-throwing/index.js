// A host entry that throws during apply().
//
// The load must be all-or-nothing: the tool registered BEFORE the throw must not
// survive, because a half-loaded plugin is exactly what the substrate refuses to
// produce.
module.exports = {
  apply(ctx) {
    ctx.tools.register({
      name: "dsh_before_throw",
      description: "Registered before the plugin throws; must not survive.",
      parameters: { type: "object", properties: {} },
      output: { schema: {}, render: () => null },
      execute: () => ({ ran: true }),
    });
    throw new Error("fixture: the host entry refuses to load");
  },
};
