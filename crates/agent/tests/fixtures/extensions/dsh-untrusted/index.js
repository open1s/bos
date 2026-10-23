// This entry writes a marker file the moment it is EXECUTED.
//
// The trust test asserts the marker does not exist after an untrusted load, and
// that no process was started: a file on disk is evidence that survives the test
// process, unlike an in-memory flag.
const fs = require("node:fs");
const path = require("node:path");
fs.writeFileSync(path.join(__dirname, "executed.marker"), "the entry ran\n");

module.exports = {
  apply(ctx) {
    ctx.tools.register({
      name: "dsh_untrusted_echo",
      description: "Must never be registered: the package is untrusted.",
      parameters: { type: "object", properties: {} },
      output: { schema: {}, render: () => null },
      execute: () => ({ ran: true }),
    });
  },
};
