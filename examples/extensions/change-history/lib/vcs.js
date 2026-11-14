'use strict';
/**
 * Change-history engine for the BSH Cordis extension.
 *
 * One model-callable tool, two backends: jj when the repository is a jj
 * workspace (the BOS convention), git otherwise. Every command runs through
 * the host's capability-gated terminal seam (ctx.terminal.run), never through
 * a child process this package opens itself, so the platform's process.execute
 * grant and audit trail cover every effect.
 *
 * The terminal seam takes SHELL SYNTAX in the host's native shell (POSIX sh on
 * Unix, PowerShell on Windows). Every argument is quoted for that dialect
 * before it reaches the command line, so a message, revision or path can never
 * be reinterpreted as shell syntax.
 *
 * The engine is deliberately dependency-free and terminal-injected: tests run
 * it against a recording mock or a real shell without a host.
 */

const fs = require('node:fs');
const path = require('node:path');

/** The actions the tool accepts, in reading-then-writing order. */
const ACTIONS = [
  'status',
  'log',
  'diff',
  'show',
  'new',
  'describe',
  'edit',
  'squash',
  'rebase',
  'restore',
  'undo',
];

/** Backend choices: auto prefers jj when both markers exist (colocated repo). */
const BACKENDS = ['auto', 'jj', 'git'];

/** One stdout/stderr stream is capped at this many characters. */
const MAX_OUTPUT_CHARS = 200000;

/** How far up the directory tree repository detection walks. */
const MAX_DETECT_DEPTH = 64;

/**
 * jj log template: one line per change, tab-separated fields. The escapes
 * stay escapes all the way into jj's own template string literals.
 * first_line() cannot contain a newline, so records never span lines.
 */
const JJ_LOG_TEMPLATE =
  'change_id.short() ++ "\\t" ++ commit_id.short() ++ "\\t" ++ ' +
  'if(description, description.first_line(), "(no description)") ++ "\\n"';

/** git log format: unit-separated fields, one line per commit. */
const GIT_LOG_FORMAT = '%h%x1f%H%x1f%s%x1f%an%x1f%aI';

/** An error whose message is safe to show the model verbatim. */
class VcsError extends Error {}

function fail(message) {
  throw new VcsError(message);
}

/** Quote one argument as a literal string in the host shell's dialect. */
function quoteArg(value, platform) {
  const text = String(value);
  if (platform === 'win32') {
    // PowerShell single-quoted literal: a doubled quote is one quote.
    return "'" + text.replace(/'/g, "''") + "'";
  }
  // POSIX sh single-quoted literal: splice an escaped quote between halves.
  return "'" + text.replace(/'/g, "'\\''") + "'";
}

/** Build one shell command line with the program and every argument quoted. */
function buildCommand(program, args, platform) {
  return [program, ...args].map((arg) => quoteArg(arg, platform)).join(' ');
}

/** Cap one output stream, reporting whether anything was dropped. */
function cap(text) {
  if (text.length <= MAX_OUTPUT_CHARS) return { text, truncated: false };
  return { text: text.slice(0, MAX_OUTPUT_CHARS), truncated: true };
}

/**
 * Detect the backend for one repository path.
 *
 * Walks ancestors and stops at the FIRST directory carrying a repo marker, so
 * a nested repository wins over its parent and a colocated jj+git repo reads
 * as jj. An explicit request must match what is found, except that git may be
 * forced inside a colocated jj repo (the git backend is still there).
 */
function detectBackend(repoPath, requested, exists) {
  let dir = path.resolve(repoPath);
  let detected = null;
  let colocatedGit = false;
  for (let depth = 0; depth < MAX_DETECT_DEPTH; depth += 1) {
    if (exists(path.join(dir, '.jj'))) {
      detected = 'jj';
      colocatedGit = exists(path.join(dir, '.git'));
      break;
    }
    if (exists(path.join(dir, '.git'))) {
      detected = 'git';
      break;
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
  if (requested === 'auto') {
    if (!detected) {
      fail(`no jj or git repository found at or above ${repoPath}`);
    }
    return detected;
  }
  if (detected === requested) return requested;
  if (requested === 'git' && detected === 'jj' && colocatedGit) return 'git';
  fail(
    `backend "${requested}" was requested, but ${
      detected ? `a ${detected} repository` : 'no repository'
    } was found at or above ${repoPath}`,
  );
}

/**
 * Run one VCS command through the terminal seam; refuse an unexpected exit.
 * opts.okCodes widens the accepted exits (git diff --no-index reports 1 when
 * the files differ, which is its success case).
 */
async function run(terminal, repo, program, args, platform, opts = {}) {
  const command = buildCommand(program, args, platform);
  let output;
  try {
    output = await terminal.run(command, repo);
  } catch (error) {
    fail(`${program} could not run in ${repo}: ${error.message}`);
  }
  const code = output && typeof output.code === 'number' ? output.code : -1;
  const stdout = cap(String((output && output.stdout) || ''));
  const stderr = cap(String((output && output.stderr) || ''));
  const okCodes = opts.okCodes || [0];
  if (!okCodes.includes(code)) {
    const detail = (stderr.text || stdout.text || '').trim();
    fail(`${program} exited with code ${code}${detail ? `: ${detail}` : ''}`);
  }
  return { stdout, stderr };
}

/** Parse jj's tab-separated log lines into structured entries. */
function parseJjLog(text) {
  return text
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => {
      // The description is the remainder: it may itself contain tabs.
      const [changeId, commitId, ...rest] = line.split('\t');
      return {
        change_id: changeId,
        commit_id: commitId,
        description: rest.join('\t'),
      };
    });
}

/** Parse git's unit-separated log lines into structured entries. */
function parseGitLog(text) {
  return text
    .split('\n')
    .filter((line) => line.length > 0)
    .map((line) => {
      const [short, commit, subject, author, date] = line.split('\x1f');
      return { short, commit, subject, author, date };
    });
}

/** Parse git porcelain v1 status (with --branch) into branch + entries. */
function parseGitStatus(text) {
  let branch = null;
  const entries = [];
  for (const line of text.split('\n')) {
    if (line.length === 0) continue;
    if (line.startsWith('## ')) {
      branch = line.slice(3);
      continue;
    }
    if (line.length < 4) continue;
    entries.push({ index: line[0], worktree: line[1], path: line.slice(3) });
  }
  return { branch, entries };
}

/** Refuse a git write that would seal an already-clean working tree. */
async function assertDirty(terminal, repo, platform, verb) {
  const git = (extra) =>
    run(terminal, repo, 'git', ['--no-pager', '-c', 'color.ui=false', ...extra], platform);
  const out = await git(['status', '--porcelain=v1']);
  if (out.stdout.text.trim().length === 0) {
    fail(`nothing to ${verb}: the working tree is clean`);
  }
}

/** The jj backend: stack-based history manipulation. */
async function jjAction(terminal, repo, args, platform) {
  const jj = (sub, extra = []) =>
    run(terminal, repo, 'jj', ['--color=never', sub, ...extra], platform);
  const logArgs = (revset, limit) => [
    '--no-graph',
    '-r',
    revset,
    '-n',
    String(limit),
    '-T',
    JJ_LOG_TEMPLATE,
  ];
  switch (args.action) {
    case 'status':
      return jj('status');
    case 'log': {
      const revset = args.revset || `ancestors(@, ${args.limit})`;
      const out = await jj('log', logArgs(revset, args.limit));
      return { ...out, data: { entries: parseJjLog(out.stdout.text) } };
    }
    case 'diff': {
      const extra = [];
      if (args.from || args.to) {
        if (!args.from || !args.to) fail('diff needs both from and to for a range');
        extra.push('--from', args.from, '--to', args.to);
      } else if (args.revision) {
        extra.push('-r', args.revision);
      }
      if (args.stat) extra.push('--stat');
      if (args.paths && args.paths.length > 0) extra.push(...args.paths);
      return jj('diff', extra);
    }
    case 'show': {
      const extra = ['-r', args.revision || '@'];
      if (args.stat) extra.push('--stat');
      return jj('show', extra);
    }
    case 'new': {
      const extra = [];
      if (args.revision) extra.push(args.revision);
      if (args.message) extra.push('-m', args.message);
      await jj('new', extra);
      const out = await jj('log', logArgs('@', 1));
      return { ...out, data: { entries: parseJjLog(out.stdout.text) } };
    }
    case 'describe':
      return jj('describe', ['-r', args.revision || '@', '-m', args.message]);
    case 'edit':
      return jj('edit', [args.revision]);
    case 'squash': {
      const extra = [];
      if (args.revision) extra.push('-r', args.revision);
      // -m and -u both keep jj from opening an editor: a bridged tool call
      // has no terminal to edit in. Without a message the parent's
      // description wins (-u), mirroring git's --amend --no-edit.
      if (args.message) extra.push('-m', args.message);
      else extra.push('-u');
      return jj('squash', extra);
    }
    case 'rebase':
      return jj('rebase', ['-r', args.revision, '-d', args.destination]);
    case 'restore': {
      const extra = ['--from', args.from || '@-'];
      if (args.paths && args.paths.length > 0) extra.push(...args.paths);
      return jj('restore', extra);
    }
    case 'undo':
      return jj('undo');
    default:
      fail(`action "${args.action}" is not supported by the jj backend`);
  }
}

/** The git backend: the same vocabulary mapped onto commit-based history. */
async function gitAction(terminal, repo, args, platform) {
  const git = (extra) =>
    run(terminal, repo, 'git', ['--no-pager', '-c', 'color.ui=false', ...extra], platform);
  switch (args.action) {
    case 'status': {
      const out = await git(['status', '--porcelain=v1', '--branch']);
      return { ...out, data: parseGitStatus(out.stdout.text) };
    }
    case 'log': {
      const extra = ['log', '-n', String(args.limit), `--format=${GIT_LOG_FORMAT}`];
      if (args.revision) extra.push(args.revision);
      const out = await git(extra);
      return { ...out, data: { entries: parseGitLog(out.stdout.text) } };
    }
    case 'diff': {
      const extra = ['diff'];
      if (args.from || args.to) {
        if (!args.from || !args.to) fail('diff needs both from and to for a range');
        extra.push(args.from, args.to);
      } else if (args.revision) {
        // rev^! is the commit against its first parent (root commits included).
        extra.push(`${args.revision}^!`);
      } else {
        extra.push('HEAD');
      }
      if (args.stat) extra.push('--stat');
      if (args.paths && args.paths.length > 0) extra.push('--', ...args.paths);
      return git(extra);
    }
    case 'show': {
      const extra = ['show'];
      if (args.stat) extra.push('--stat');
      extra.push(args.revision || 'HEAD');
      return git(extra);
    }
    case 'new': {
      if (!args.message) {
        fail('new needs a message on the git backend: it seals the working tree into a commit');
      }
      await assertDirty(terminal, repo, platform, 'commit');
      await git(['add', '-A']);
      await git(['commit', '-m', args.message]);
      const out = await git(['log', '-n', '1', `--format=${GIT_LOG_FORMAT}`]);
      return { ...out, data: { entries: parseGitLog(out.stdout.text) } };
    }
    case 'describe': {
      if (args.revision && args.revision !== 'HEAD') {
        fail('the git backend can only reword HEAD; omit revision (or pass HEAD)');
      }
      return git(['commit', '--amend', '-m', args.message]);
    }
    case 'edit': {
      const out = await git(['checkout', args.revision]);
      let detached = false;
      try {
        await git(['symbolic-ref', '-q', 'HEAD']);
      } catch {
        detached = true;
      }
      return { ...out, data: { detached } };
    }
    case 'squash': {
      if (args.revision && args.revision !== 'HEAD') {
        fail('the git backend folds the working tree into HEAD only; omit revision (or pass HEAD)');
      }
      await assertDirty(terminal, repo, platform, 'squash');
      await git(['add', '-A']);
      const extra = ['commit', '--amend'];
      extra.push(...(args.message ? ['-m', args.message] : ['--no-edit']));
      return git(extra);
    }
    case 'rebase': {
      if (args.revision) {
        const parents = await git(['rev-list', '--parents', '-n', '1', args.revision]);
        const fields = parents.stdout.text.trim().split(/\s+/);
        if (fields.length > 2) {
          fail('the git backend cannot rebase a merge commit; rebase it manually');
        }
        const out = await git([
          'rebase',
          '--onto',
          args.destination,
          `${args.revision}^`,
          args.revision,
        ]);
        return {
          ...out,
          data: { detached: true, note: 'HEAD is detached at the rebased commit' },
        };
      }
      return git(['rebase', args.destination]);
    }
    case 'restore': {
      const paths = args.paths && args.paths.length > 0 ? args.paths : ['.'];
      return git([
        'restore',
        `--source=${args.from || 'HEAD'}`,
        '--worktree',
        '--',
        ...paths,
      ]);
    }
    case 'undo':
      fail(
        'the git backend has no operation log to undo; use restore to recover files from a revision, or edit to return to a commit',
      );
    default:
      fail(`action "${args.action}" is not supported by the git backend`);
  }
}

/** Validate one call's arguments; returns the normalized shape. */
function validate(rawArgs) {
  const args = rawArgs && typeof rawArgs === 'object' ? rawArgs : {};
  if (!ACTIONS.includes(args.action)) {
    fail(`action must be one of ${ACTIONS.join(', ')}; got ${JSON.stringify(args.action)}`);
  }
  if (typeof args.path !== 'string' || args.path.length === 0) {
    fail('path is required: the absolute path of the repository (usually the session workspace)');
  }
  if (!path.isAbsolute(args.path)) {
    fail(`path must be absolute so the capability scope and the command agree; got ${args.path}`);
  }
  const backend = args.backend === undefined ? 'auto' : args.backend;
  if (!BACKENDS.includes(backend)) {
    fail(`backend must be one of ${BACKENDS.join(', ')}; got ${JSON.stringify(args.backend)}`);
  }
  const limit = args.limit === undefined ? 20 : args.limit;
  if (!Number.isInteger(limit) || limit < 1 || limit > 200) {
    fail('limit must be an integer between 1 and 200');
  }
  for (const key of ['revision', 'destination', 'from', 'to', 'message', 'revset']) {
    if (args[key] !== undefined && typeof args[key] !== 'string') {
      fail(`${key} must be a string`);
    }
  }
  if (
    args.paths !== undefined &&
    (!Array.isArray(args.paths) || args.paths.some((p) => typeof p !== 'string'))
  ) {
    fail('paths must be an array of strings');
  }
  if (args.stat !== undefined && typeof args.stat !== 'boolean') {
    fail('stat must be a boolean');
  }
  if (args.action === 'describe' && !args.message) fail('describe needs a message');
  if (args.action === 'edit' && !args.revision) fail('edit needs a revision');
  if (args.action === 'rebase' && !args.destination) fail('rebase needs a destination');
  return { ...args, backend, limit };
}

/**
 * Run one change_history call.
 *
 * terminal is the host seam ({run(command, cwd)}); deps injects platform and
 * filesystem probes for tests. Returns the result envelope; throws VcsError
 * with a model-readable message on any refusal or command failure.
 */
async function changeHistory(terminal, rawArgs, deps = {}) {
  const platform = deps.platform || process.platform;
  const exists = deps.exists || fs.existsSync;
  const args = validate(rawArgs);
  if (!exists(args.path)) fail(`path does not exist: ${args.path}`);
  const backend = detectBackend(args.path, args.backend, exists);
  if (backend === 'git' && args.revset) {
    fail('revset is a jj query; on the git backend pass revision instead');
  }
  const result =
    backend === 'jj'
      ? await jjAction(terminal, args.path, args, platform)
      : await gitAction(terminal, args.path, args, platform);
  const envelope = {
    ok: true,
    backend,
    action: args.action,
    repo: args.path,
    output: result.stdout.text,
    truncated: result.stdout.truncated || result.stderr.truncated,
  };
  if (result.stderr.text.trim().length > 0) envelope.stderr = result.stderr.text;
  if (result.data) envelope.data = result.data;
  return envelope;
}

/** The one-paragraph description the model sees. */
const TOOL_DESCRIPTION =
  "Inspect and manipulate a repository's change history, backed by jj when the " +
  'workspace is a jj repository and git otherwise (backend: auto|jj|git). ' +
  'Read actions: status, log, diff, show. Write actions: new (jj: start a change; ' +
  'git: commit the working tree, message required), describe (set a message; git: ' +
  'HEAD only), edit (check out a revision), squash (fold into the parent; git: into ' +
  'HEAD), rebase (needs destination; git: one commit or the current branch), restore ' +
  '(recover files from a revision), undo (jj only: reverse the last operation). ' +
  'path must be the absolute repository path, usually the session workspace. ' +
  'revision is a jj change id or a git commit; log/diff/show default to the working ' +
  'copy. Results carry output text plus structured data for status and log.';

/** The JSON Schema the host advertises for the tool. */
const TOOL_SCHEMA = {
  type: 'object',
  properties: {
    action: {
      type: 'string',
      enum: ACTIONS,
      description: 'The history operation to run.',
    },
    path: {
      type: 'string',
      description:
        'Absolute path of the repository (usually the session workspace). Required.',
    },
    backend: {
      type: 'string',
      enum: BACKENDS,
      description: 'Force a backend; auto prefers jj when both markers exist.',
    },
    revision: {
      type: 'string',
      description:
        'jj change id / revset or git commit. log: start point; diff/show: the change; new: parent; describe/edit/squash/rebase: the target.',
    },
    destination: {
      type: 'string',
      description: 'rebase: where to move the change (required for rebase).',
    },
    from: {
      type: 'string',
      description: 'diff: range start (with to); restore: source revision (default parent/HEAD).',
    },
    to: { type: 'string', description: 'diff: range end (with from).' },
    message: {
      type: 'string',
      description: 'new/describe/squash: the change description. Required for describe and git new.',
    },
    paths: {
      type: 'array',
      items: { type: 'string' },
      description: 'diff/restore: repository-relative path filter.',
    },
    limit: {
      type: 'integer',
      minimum: 1,
      maximum: 200,
      description: 'log: maximum entries (default 20).',
    },
    revset: {
      type: 'string',
      description: 'log: jj revset override (jj backend only).',
    },
    stat: {
      type: 'boolean',
      description: 'diff/show: summary statistics instead of the full patch.',
    },
  },
  required: ['action', 'path'],
};

module.exports = {
  ACTIONS,
  BACKENDS,
  MAX_OUTPUT_CHARS,
  TOOL_DESCRIPTION,
  TOOL_SCHEMA,
  VcsError,
  buildCommand,
  cap,
  changeHistory,
  detectBackend,
  parseGitLog,
  parseGitStatus,
  parseJjLog,
  quoteArg,
  run,
  validate,
};
