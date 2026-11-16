'use strict';
/**
 * Tests for the change-history engine.
 *
 * Unit tests run against a recording mock terminal; integration tests run the
 * real command lines through the host shell dialect (sh -c on Unix,
 * PowerShell on Windows) against scratch jj and git repositories, exactly the
 * way ctx.terminal.run executes them in production. Integration tests skip
 * themselves when the backend binary is not installed.
 *
 * Run: node --test test/
 */

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const vcs = require('../lib/vcs.js');

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/** A terminal that records commands and replays canned outputs. */
function mockTerminal(outputs = []) {
  const calls = [];
  let index = 0;
  return {
    calls,
    run(command, cwd) {
      calls.push({ command, cwd });
      const next = outputs[index] || { stdout: '', stderr: '', code: 0 };
      index += 1;
      return Promise.resolve(next);
    },
  };
}

/** The host terminal dialect: sh -c on Unix, PowerShell on Windows. */
function shellTerminal() {
  return {
    run(command, cwd) {
      const result =
        process.platform === 'win32'
          ? spawnSync('powershell.exe', ['-NoProfile', '-Command', command], {
              cwd,
              encoding: 'utf8',
              timeout: 30000,
            })
          : spawnSync('sh', ['-c', command], { cwd, encoding: 'utf8', timeout: 30000 });
      return Promise.resolve({
        stdout: result.stdout || '',
        stderr: result.stderr || String(result.error || ''),
        code: typeof result.status === 'number' ? result.status : -1,
      });
    },
  };
}

function hasBinary(name) {
  const result = spawnSync(name, ['--version'], { encoding: 'utf8' });
  return result.status === 0;
}

function scratchRepo(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'change-history-' + label + '-'));
}

function gitInit(repo) {
  const run = (args) => {
    const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8' });
    assert.equal(result.status, 0, 'git ' + args.join(' ') + ': ' + result.stderr);
  };
  run(['init', '-q']);
  run(['symbolic-ref', 'HEAD', 'refs/heads/main']);
  run(['config', 'user.name', 'Change History Test']);
  run(['config', 'user.email', 'test@example.invalid']);
  run(['config', 'commit.gpgsign', 'false']);
  fs.writeFileSync(path.join(repo, 'README.md'), 'seed\n');
  run(['add', '-A']);
  run(['commit', '-qm', 'first commit']);
}

function jjInit(repo) {
  const parent = path.dirname(repo);
  const name = path.basename(repo);
  const init = spawnSync('jj', ['git', 'init', name], { cwd: parent, encoding: 'utf8' });
  assert.equal(init.status, 0, 'jj git init: ' + init.stderr);
  for (const [key, value] of [
    ['user.name', 'Change History Test'],
    ['user.email', 'test@example.invalid'],
  ]) {
    const result = spawnSync('jj', ['config', 'set', '--repo', key, value], {
      cwd: repo,
      encoding: 'utf8',
    });
    assert.equal(result.status, 0, 'jj config: ' + result.stderr);
  }
}

const call = (terminal, args) => vcs.changeHistory(terminal, args);

/** jj reports mutations on stderr; the envelope keeps both streams. */
const text = (out) => out.output + (out.stderr || '');

// ---------------------------------------------------------------------------
// quoting and command building
// ---------------------------------------------------------------------------

test('posix quoting neutralizes shell metacharacters', () => {
  assert.equal(vcs.quoteArg('plain', 'linux'), "'plain'");
  assert.equal(vcs.quoteArg("it's", 'linux'), "'it'\\''s'");
  assert.equal(vcs.quoteArg('a b;rm -rf /', 'linux'), "'a b;rm -rf /'");
  assert.equal(vcs.quoteArg('@ $(echo x) `id`', 'linux'), "'@ $(echo x) `id`'");
});

test('powershell quoting doubles embedded single quotes', () => {
  assert.equal(vcs.quoteArg("it's", 'win32'), "'it''s'");
  assert.equal(vcs.quoteArg('a b', 'win32'), "'a b'");
});

test('buildCommand quotes the program and every argument', () => {
  const command = vcs.buildCommand('jj', ['describe', '-m', "it's"], 'linux');
  assert.equal(command, "'jj' 'describe' '-m' 'it'\\''s'");
});

// ---------------------------------------------------------------------------
// backend detection
// ---------------------------------------------------------------------------

/**
 * An exists() probe over a fixed set of marker directories. Each marker's
 * parent directory is included too, so changeHistory's path-exists probe
 * passes for the repository root the markers live in.
 */
function markers(paths) {
  const set = new Set(paths);
  for (const p of paths) set.add(path.dirname(p));
  return (p) => set.has(p);
}

test('detection prefers jj at the same level (colocated)', () => {
  const exists = markers(['/repo/.jj', '/repo/.git']);
  assert.equal(vcs.detectBackend('/repo', 'auto', exists), 'jj');
});

test('detection finds git when jj is absent', () => {
  const exists = markers(['/repo/.git']);
  assert.equal(vcs.detectBackend('/repo', 'auto', exists), 'git');
});

test('detection walks ancestors and stops at the nearest repo', () => {
  const exists = markers(['/parent/.jj', '/parent/child/.git']);
  assert.equal(vcs.detectBackend('/parent/child', 'auto', exists), 'git');
  assert.equal(vcs.detectBackend('/parent', 'auto', exists), 'jj');
});

test('detection refuses a directory with no repository', () => {
  assert.throws(() => vcs.detectBackend('/plain', 'auto', markers([])), /no jj or git repository/);
});

test('an explicit backend must match, except git inside a colocated repo', () => {
  const colocated = markers(['/repo/.jj', '/repo/.git']);
  assert.equal(vcs.detectBackend('/repo', 'git', colocated), 'git');
  assert.equal(vcs.detectBackend('/repo', 'jj', colocated), 'jj');
  const gitOnly = markers(['/repo/.git']);
  assert.throws(() => vcs.detectBackend('/repo', 'jj', gitOnly), /backend "jj" was requested/);
  const jjOnly = markers(['/repo/.jj']);
  assert.throws(() => vcs.detectBackend('/repo', 'git', jjOnly), /backend "git" was requested/);
});

// ---------------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------------

test('jj log lines parse, keeping tabs inside the description', () => {
  const entries = vcs.parseJjLog('abc\tdef\tfirst line\nxyz\tuv\twith\ttabs\n');
  assert.deepEqual(entries, [
    { change_id: 'abc', commit_id: 'def', description: 'first line' },
    { change_id: 'xyz', commit_id: 'uv', description: 'with\ttabs' },
  ]);
});

test('git log lines parse on the unit separator', () => {
  const entries = vcs.parseGitLog('abc\x1fabc123\x1fsubject\x1fAuthor\x1f2026-11-14T16:00:00+08:00\n');
  assert.deepEqual(entries, [
    { short: 'abc', commit: 'abc123', subject: 'subject', author: 'Author', date: '2026-11-14T16:00:00+08:00' },
  ]);
});

test('git porcelain status parses branch and entries', () => {
  const data = vcs.parseGitStatus('## main...origin/main [ahead 1]\n M a.txt\n?? new.txt\nR  old.txt -> renamed.txt\n');
  assert.equal(data.branch, 'main...origin/main [ahead 1]');
  assert.deepEqual(data.entries, [
    { index: ' ', worktree: 'M', path: 'a.txt' },
    { index: '?', worktree: '?', path: 'new.txt' },
    { index: 'R', worktree: ' ', path: 'old.txt -> renamed.txt' },
  ]);
});

test('output caps report truncation', () => {
  const long = 'x'.repeat(vcs.MAX_OUTPUT_CHARS + 10);
  const capped = vcs.cap(long);
  assert.equal(capped.truncated, true);
  assert.equal(capped.text.length, vcs.MAX_OUTPUT_CHARS);
  assert.equal(vcs.cap('short').truncated, false);
});

// ---------------------------------------------------------------------------
// argument validation
// ---------------------------------------------------------------------------

test('validation refuses a missing or unknown action', () => {
  assert.throws(() => vcs.validate({ path: '/repo' }), /action must be one of/);
  assert.throws(() => vcs.validate({ action: 'push', path: '/repo' }), /action must be one of/);
});

test('validation requires an absolute path', () => {
  assert.throws(() => vcs.validate({ action: 'status' }), /path is required/);
  assert.throws(() => vcs.validate({ action: 'status', path: 'relative' }), /path must be absolute/);
});

test('validation bounds limit and checks field types', () => {
  const base = { action: 'log', path: '/repo' };
  for (const limit of [0, 201, 1.5, 'ten']) {
    assert.throws(() => vcs.validate({ ...base, limit }), /limit must be an integer/);
  }
  assert.throws(() => vcs.validate({ ...base, revision: 4 }), /revision must be a string/);
  assert.throws(() => vcs.validate({ ...base, paths: 'a.txt' }), /paths must be an array/);
  assert.throws(() => vcs.validate({ ...base, paths: [1] }), /paths must be an array/);
  assert.throws(() => vcs.validate({ ...base, stat: 'yes' }), /stat must be a boolean/);
  assert.throws(() => vcs.validate({ ...base, backend: 'hg' }), /backend must be one of/);
});

test('validation enforces per-action requirements', () => {
  assert.throws(() => vcs.validate({ action: 'describe', path: '/repo' }), /describe needs a message/);
  assert.throws(() => vcs.validate({ action: 'edit', path: '/repo' }), /edit needs a revision/);
  assert.throws(() => vcs.validate({ action: 'rebase', path: '/repo' }), /rebase needs a destination/);
});

// ---------------------------------------------------------------------------
// dispatch through a mock terminal
// ---------------------------------------------------------------------------

test('the envelope names backend, action and repo', async () => {
  const terminal = mockTerminal([{ stdout: '## main\n', stderr: '', code: 0 }]);
  const exists = markers(['/repo/.git']);
  const out = await vcs.changeHistory(terminal, { action: 'status', path: '/repo' }, { exists });
  assert.equal(out.ok, true);
  assert.equal(out.backend, 'git');
  assert.equal(out.action, 'status');
  assert.equal(out.repo, '/repo');
  assert.equal(out.data.branch, 'main');
  assert.equal(terminal.calls[0].cwd, '/repo');
});

test('jj reads carry --color=never and the log template', async () => {
  const terminal = mockTerminal([{ stdout: 'abc\tdef\tmsg\n', stderr: '', code: 0 }]);
  const exists = markers(['/repo/.jj']);
  const out = await vcs.changeHistory(
    terminal,
    { action: 'log', path: '/repo', limit: 5 },
    { exists },
  );
  assert.equal(out.backend, 'jj');
  assert.deepEqual(out.data.entries, [{ change_id: 'abc', commit_id: 'def', description: 'msg' }]);
  const command = terminal.calls[0].command;
  assert.match(command, /--color=never/);
  assert.match(command, /'-n' '5'/);
  assert.match(command, /ancestors\(@, 5\)/);
});

test('a non-zero exit surfaces stderr as the refusal', async () => {
  const terminal = mockTerminal([{ stdout: '', stderr: 'fatal: not a git repository', code: 128 }]);
  const exists = markers(['/repo/.git']);
  await assert.rejects(
    vcs.changeHistory(terminal, { action: 'status', path: '/repo' }, { exists }),
    /exited with code 128: fatal: not a git repository/,
  );
});

test('a terminal failure is a named refusal', async () => {
  const terminal = { run: () => Promise.reject(new Error('denied by policy')) };
  const exists = markers(['/repo/.git']);
  await assert.rejects(
    vcs.changeHistory(terminal, { action: 'status', path: '/repo' }, { exists }),
    /could not run in \/repo: denied by policy/,
  );
});

test('git undo refuses with a recovery hint', async () => {
  const terminal = mockTerminal();
  const exists = markers(['/repo/.git']);
  await assert.rejects(
    vcs.changeHistory(terminal, { action: 'undo', path: '/repo' }, { exists }),
    /no operation log to undo/,
  );
  assert.equal(terminal.calls.length, 0);
});

test('a jj revset on the git backend refuses', async () => {
  const terminal = mockTerminal();
  const exists = markers(['/repo/.git']);
  await assert.rejects(
    vcs.changeHistory(terminal, { action: 'log', path: '/repo', revset:'@' }, { exists }),
    /revset is a jj query/,
  );
});

test('git new needs a message and a dirty tree', async () => {
  const exists = markers(['/repo/.git']);
  const noMessage = mockTerminal();
  await assert.rejects(
    vcs.changeHistory(noMessage, { action: 'new', path: '/repo' }, { exists }),
    /new needs a message/,
  );
  const clean = mockTerminal([{ stdout: '', stderr: '', code: 0 }]);
  await assert.rejects(
    vcs.changeHistory(clean, { action: 'new', path: '/repo', message: 'm' }, { exists }),
    /nothing to commit: the working tree is clean/,
  );
});

test('git describe and squash refuse non-HEAD revisions', async () => {
  const exists = markers(['/repo/.git']);
  await assert.rejects(
    vcs.changeHistory(mockTerminal(), { action: 'describe', path: '/repo', revision: 'abc123', message: 'm' }, { exists }),
    /can only reword HEAD/,
  );
  await assert.rejects(
    vcs.changeHistory(mockTerminal(), { action: 'squash', path: '/repo', revision: 'abc123' }, { exists }),
    /folds the working tree into HEAD only/,
  );
});

test('diff ranges need both ends on either backend', async () => {
  for (const backend of ['jj', 'git']) {
    const exists = markers(['/repo/.' + backend]);
    await assert.rejects(
      vcs.changeHistory(mockTerminal(), { action: 'diff', path: '/repo', from: 'a' }, { exists }),
      /diff needs both from and to/,
    );
  }
});

// ---------------------------------------------------------------------------
// the Cordis entry registers a callable tool
// ---------------------------------------------------------------------------

test('apply(ctx) registers the tool family with schemas and bodies', async () => {
  const registered = [];
  const panels = [];
  const terminal = mockTerminal([{ stdout: '## main\n', stderr: '', code: 0 }]);
  const ctx = {
    tools: { register: (definition) => { registered.push(definition); } },
    panels: { register: (definition) => { panels.push(definition); } },
    terminal,
  };
  require('../index.js').apply(ctx);
  const names = registered.map((definition) => definition.name);
  assert.deepEqual(names, ['change_history', 'refresh', 'commit', 'file']);
  assert.deepEqual(panels.map((definition) => definition.name), ['git']);
  assert.equal(typeof panels[0].render, 'function');
  for (const definition of registered) {
    assert.equal(typeof definition.execute, 'function', definition.name);
    assert.equal(definition.parameters.type, 'object', definition.name);
  }
  const history = registered[0];
  assert.deepEqual(history.parameters.required, ['action', 'path']);
  const exists = markers(['/repo/.git']);
  // The entry closes over ctx.terminal; exercise it through the engine seam.
  const out = await vcs.changeHistory(terminal, { action: 'status', path: '/repo' }, { exists });
  assert.equal(out.ok, true);
});

// ---------------------------------------------------------------------------
// integration: a real git repository through the real shell dialect
// ---------------------------------------------------------------------------

test('git integration: the full history vocabulary', async (t) => {
  if (!hasBinary('git')) return t.skip('git is not installed');
  const repo = scratchRepo('git');
  t.after(() => fs.rmSync(repo, { recursive: true, force: true }));
  gitInit(repo);
  const terminal = shellTerminal();
  const args = (extra) => ({ path: repo, ...extra });

  // status: clean seed
  let out = await call(terminal, args({ action: 'status' }));
  assert.equal(out.backend, 'git');
  assert.equal(out.data.branch, 'main');
  assert.deepEqual(out.data.entries, []);

  // new: message required, then seals the working tree
  fs.writeFileSync(path.join(repo, 'change.txt'), 'one\n');
  out = await call(terminal, args({ action: 'status' }));
  assert.deepEqual(out.data.entries, [{ index: '?', worktree: '?', path: 'change.txt' }]);
  await assert.rejects(call(terminal, args({ action: 'new' })), /new needs a message/);
  out = await call(terminal, args({ action: 'new', message: 'second commit' }));
  assert.equal(out.data.entries[0].subject, 'second commit');

  // log: both commits, newest first
  out = await call(terminal, args({ action: 'log', limit: 5 }));
  assert.equal(out.data.entries.length, 2);
  assert.equal(out.data.entries[0].subject, 'second commit');
  assert.equal(out.data.entries[1].subject, 'first commit');

  // describe: reword HEAD
  await call(terminal, args({ action: 'describe', message: 'second commit reworded' }));
  out = await call(terminal, args({ action: 'log', limit: 1 }));
  assert.equal(out.data.entries[0].subject, 'second commit reworded');

  // diff: working tree against HEAD, full and stat
  fs.writeFileSync(path.join(repo, 'change.txt'), 'two\n');
  out = await call(terminal, args({ action: 'diff' }));
  assert.match(out.output, /\+two/);
  out = await call(terminal, args({ action: 'diff', stat: true, paths: ['change.txt'] }));
  assert.match(out.output, /change\.txt/);

  // show: one commit's metadata and patch
  out = await call(terminal, args({ action: 'show', stat: true }));
  assert.match(out.output, /second commit reworded/);

  // squash: fold the working tree into HEAD
  out = await call(terminal, args({ action: 'squash', message: 'squashed into second' }));
  assert.equal(out.ok, true);
  out = await call(terminal, args({ action: 'log', limit: 5 }));
  assert.equal(out.data.entries.length, 2);
  assert.equal(out.data.entries[0].subject, 'squashed into second');

  // restore: recover a file from HEAD
  fs.writeFileSync(path.join(repo, 'change.txt'), 'clobbered\n');
  await call(terminal, args({ action: 'restore', paths: ['change.txt'] }));
  assert.equal(fs.readFileSync(path.join(repo, 'change.txt'), 'utf8'), 'two\n');

  // edit: detached at a commit, back on the branch
  out = await call(terminal, args({ action: 'log', limit: 5 }));
  const firstCommit = out.data.entries[1].commit;
  out = await call(terminal, args({ action: 'edit', revision: firstCommit }));
  assert.equal(out.data.detached, true);
  out = await call(terminal, args({ action: 'edit', revision: 'main' }));
  assert.equal(out.data.detached, false);

  // rebase: move one commit onto an earlier destination
  fs.writeFileSync(path.join(repo, 'third.txt'), 'three\n');
  await call(terminal, args({ action: 'new', message: 'third commit' }));
  out = await call(terminal, args({ action: 'log', limit: 5 }));
  const third = out.data.entries[0].commit;
  const second = out.data.entries[1].commit;
  out = await call(terminal, args({ action: 'rebase', revision: third, destination: firstCommit }));
  assert.equal(out.data.detached, true);
  out = await call(terminal, args({ action: 'log', limit: 5 }));
  assert.deepEqual(out.data.entries.map((e) => e.subject), ['third commit', 'first commit']);
  assert.notEqual(out.data.entries[0].commit, third);
  assert.ok(second); // the skipped commit is still reachable by id

  // undo: honest refusal on git
  await assert.rejects(call(terminal, args({ action: 'undo' })), /no operation log to undo/);

  // quoting: a hostile message round-trips verbatim
  const hostile = "it's a \"test\"; $(echo pwned) @head";
  await call(terminal, args({ action: 'edit', revision: 'main' }));
  await call(terminal, args({ action: 'describe', message: hostile }));
  out = await call(terminal, args({ action: 'log', limit: 1 }));
  assert.equal(out.data.entries[0].subject, hostile);

  // new on a clean tree refuses
  await assert.rejects(
    call(terminal, args({ action: 'new', message: 'empty' })),
    /nothing to commit: the working tree is clean/,
  );
});

// ---------------------------------------------------------------------------
// integration: a real jj repository through the real shell dialect
// ---------------------------------------------------------------------------

test('jj integration: the stack vocabulary', async (t) => {
  if (!hasBinary('jj')) return t.skip('jj is not installed');
  if (!hasBinary('git')) return t.skip('jj git backend needs git');
  const parent = scratchRepo('jj');
  const repo = path.join(parent, 'repo');
  t.after(() => fs.rmSync(parent, { recursive: true, force: true }));
  jjInit(repo);
  const terminal = shellTerminal();
  const args = (extra) => ({ path: repo, ...extra });

  // status detects jj
  let out = await call(terminal, args({ action: 'status' }));
  assert.equal(out.backend, 'jj');

  // describe the working copy, then start a new change on top
  fs.writeFileSync(path.join(repo, 'a.txt'), 'one\n');
  await call(terminal, args({ action: 'describe', message: 'first change' }));
  out = await call(terminal, args({ action: 'new', message: 'second change' }));
  assert.equal(out.data.entries[0].description, 'second change');

  // log: the stack, newest first
  fs.writeFileSync(path.join(repo, 'b.txt'), 'two\n');
  out = await call(terminal, args({ action: 'log', limit: 5 }));
  assert.equal(out.data.entries[0].description, 'second change');
  assert.equal(out.data.entries[1].description, 'first change');

  // diff: the working copy against its parent
  out = await call(terminal, args({ action: 'diff' }));
  assert.match(out.output, /b\.txt/);
  out = await call(terminal, args({ action: 'diff', stat: true }));
  assert.match(out.output, /b\.txt/);

  // show: the parent change
  out = await call(terminal, args({ action: 'show', revision: '@-', stat: true }));
  assert.match(out.output, /first change/);

  // squash: fold the working copy into its parent
  out = await call(terminal, args({ action: 'squash' }));
  assert.equal(out.ok, true);
  out = await call(terminal, args({ action: 'log', limit: 3 }));
  assert.equal(out.data.entries[0].description, '(no description)');
  assert.equal(out.data.entries[1].description, 'first change');

  // edit: move the working copy onto the parent change
  out = await call(terminal, args({ action: 'edit', revision: '@-' }));
  assert.equal(out.ok, true);
  out = await call(terminal, args({ action: 'log', limit: 1 }));
  assert.equal(out.data.entries[0].description, 'first change');

  // rebase: move a change to the root
  await call(terminal, args({ action: 'new', message: 'third change' }));
  fs.writeFileSync(path.join(repo, 'c.txt'), 'three\n');
  out = await call(terminal, args({ action: 'rebase', revision: '@', destination: 'root()' }));
  assert.match(text(out), /Rebased/);

  // undo: reverse the rebase
  out = await call(terminal, args({ action: 'undo' }));
  assert.match(text(out), /Undid operation/);

  // restore: recover a file from the parent
  fs.writeFileSync(path.join(repo, 'a.txt'), 'clobbered\n');
  out = await call(terminal, args({ action: 'restore', from: '@-', paths: ['a.txt'] }));
  assert.equal(out.ok, true);
  assert.equal(fs.readFileSync(path.join(repo, 'a.txt'), 'utf8'), 'one\n');

  // revset override
  out = await call(terminal, args({ action: 'log', revset: 'root()', limit: 1 }));
  assert.equal(out.data.entries.length, 1);
  assert.match(out.data.entries[0].change_id, /^z+$/);

  // forcing the git backend inside the colocated repo is allowed
  out = await call(terminal, args({ action: 'status', backend: 'git' }));
  assert.equal(out.backend, 'git');

  // quoting: a hostile description round-trips verbatim
  const hostile = "it's a \"test\"; $(echo pwned) @head";
  await call(terminal, args({ action: 'describe', message: hostile }));
  out = await call(terminal, args({ action: 'log', limit: 1 }));
  assert.equal(out.data.entries[0].description, hostile);
});
