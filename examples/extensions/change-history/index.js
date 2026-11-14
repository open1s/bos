'use strict';
/**
 * Change History — a first-party BSH Cordis extension.
 *
 * Registers the change_history tool (history inspection and manipulation)
 * and the Git panel's three action tools: git (the live card), commit (the
 * message field) and file (a change row's diff). All effects run through
 * ctx.terminal.run, the host's capability-gated terminal seam, so the
 * process.execute grant and the audit trail cover every command. The engines
 * live in lib/vcs.js and lib/panel.js and are terminal-injected for tests.
 */

const { changeHistory, TOOL_DESCRIPTION, TOOL_SCHEMA } = require('./lib/vcs.js');
const panel = require('./lib/panel.js');

/** The repository argument every panel tool accepts in place of the workspace. */
const PATH_PROPERTY = {
  type: 'string',
  description: 'Absolute repository path; defaults to the call workspace.',
};

module.exports = {
  apply(ctx) {
    ctx.tools.register({
      name: 'change_history',
      description: TOOL_DESCRIPTION,
      parameters: TOOL_SCHEMA,
      output: { schema: {}, render: () => null },
      async execute(args) {
        return changeHistory(ctx.terminal, args || {});
      },
    });
    ctx.panels.register({
      name: 'git',
      description: 'Working-copy changes and the commit graph, jj or git.',
      render(args, meta) {
        return panel.gitPanel(ctx.terminal, args, meta);
      },
    });
    ctx.tools.register({
      name: 'refresh',
      description:
        'The Git panel card: working-copy changes with line stats and the commit graph. ' +
        'Returns a declarative UI document; value selects the branch filter.',
      parameters: {
        type: 'object',
        properties: {
          path: PATH_PROPERTY,
          backend: { type: 'string', enum: ['auto', 'jj', 'git'] },
          value: { type: 'string', description: 'Branch filter selected in the panel.' },
        },
      },
      output: { schema: {}, render: () => null },
      async execute(args, meta) {
        return panel.gitPanel(ctx.terminal, args || {}, meta);
      },
    });
    ctx.tools.register({
      name: 'commit',
      description:
        'Commit the working copy with the given message (jj commit / git add+commit), ' +
        'then return the refreshed Git panel card.',
      parameters: {
        type: 'object',
        properties: {
          path: PATH_PROPERTY,
          value: { type: 'string', description: 'The commit message.' },
        },
        required: ['value'],
      },
      output: { schema: {}, render: () => null },
      async execute(args, meta) {
        return panel.commitAction(ctx.terminal, args || {}, meta);
      },
    });
    ctx.tools.register({
      name: 'file',
      description: 'Show one changed file\'s diff from the Git panel, by row index.',
      parameters: {
        type: 'object',
        properties: {
          path: PATH_PROPERTY,
          index: { type: 'integer', minimum: 0, description: 'The change row index.' },
        },
        required: ['index'],
      },
      output: { schema: {}, render: () => null },
      async execute(args, meta) {
        return panel.fileAction(ctx.terminal, args || {}, meta);
      },
    });
  },
};
