'use strict';
/**
 * Tests for the Git panel engine: the high-fidelity card (file tree, stats,
 * timeline rail, chips, tabs), the commit/file actions and the branch
 * filter, against real scratch repositories through the host shell dialect.
 * Run: node --test
 */

const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const panel = require('../lib/panel.js');

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
  return spawnSync(name, ['--version'], { encoding: 'utf8' }).status === 0;
}

function scratchRepo(label) {
  return fs.mkdtempSync(path.join(os.tmpdir(), 'change-history-panel-' + label + '-'));
}

function gitInit(repo) {
  const run = (args) => {
    const result = spawnSync('git', args, { cwd: repo, encoding: 'utf8' });
    assert.equal(result.status, 0, 'git ' + args.join(' ') + ': ' + result.stderr);
  };
  run(['init', '-q']);
  run(['symbolic-ref', 'HEAD', 'refs/heads/main']);
  run(['config', 'user.name', 'Panel Test']);
  run(['config', 'user.email', 'test@example.invalid']);
  run(['config', 'commit.gpgsign', 'false']);
  fs.writeFileSync(path.join(repo, 'README.md'), 'seed\n');
  run(['add', '-A']);
  run(['commit', '-qm', 'first commit']);
}

function jjInit(repo) {
  const parent = path.dirname(repo);
  const init = spawnSync('jj', ['git', 'init', path.basename(repo)], {
    cwd: parent,
    encoding: 'utf8',
  });
  assert.equal(init.status, 0, 'jj git init: ' + init.stderr);
  for (const [key, value] of [
    ['user.name', 'Panel Test'],
    ['user.email', 'test@example.invalid'],
  ]) {
    const result = spawnSync('jj', ['config', 'set', '--repo', key, value], {
      cwd: repo,
      encoding: 'utf8',
    });
    assert.equal(result.status, 0, 'jj config: ' + result.stderr);
  }
}

/** Walk a card and assert the host's UI limits hold. */
function assertUiLimits(doc) {
  assert.equal(doc.schema_version, '1.0');
  let nodes = 0;
  let text = 0;
  const leaf = (value) => {
    if (typeof value !== 'string') return;
    assert.ok(value.length <= 16 * 1024, 'leaf limit');
    text += value.length;
  };
  const badges = (list) => {
    for (const badge of list || []) {
      nodes += 1;
      leaf(badge.text);
    }
  };
  const treeNode = (node, depth) => {
    nodes += 1;
    assert.ok(depth <= 32, 'depth limit');
    leaf(node.label);
    leaf(node.detail);
    badges(node.badges);
    for (const child of node.children || []) treeNode(child, depth + 1);
  };
  const walk = (node, depth) => {
    nodes += 1;
    assert.ok(depth <= 32, 'depth limit');
    for (const key of ['text', 'value', 'placeholder', 'label', 'title', 'detail']) {
      leaf(node[key]);
    }
    badges(node.badges);
    for (const child of node.children || []) walk(child, depth + 1);
    for (const child of node.tabs || []) walk(child.content, depth + 1);
    for (const child of node.nodes || []) treeNode(child, depth + 1);
    for (const item of node.items || []) {
      nodes += 1;
      leaf(item.title);
      leaf(item.detail);
      badges(item.badges);
    }
    for (const option of node.options || []) {
      nodes += 1;
      leaf(option.label);
      leaf(option.value);
    }
  };
  walk(doc.root, 1);
  assert.ok(nodes <= 1000, 'node limit: ' + nodes);
  assert.ok(text <= 256 * 1024, 'text limit: ' + text);
}

/** Find the first node of a type, depth-first (tabs and trees included). */
function findNode(node, type) {
  if (node.type === type) return node;
  for (const child of node.children || []) {
    const hit = findNode(child, type);
    if (hit) return hit;
  }
  for (const child of node.tabs || []) {
    const hit = findNode(child.content, type);
    if (hit) return hit;
  }
  for (const child of node.nodes || []) {
    const hit = findNode(child, type);
    if (hit) return hit;
  }
  return null;
}

/** Collect every tree leaf (a node with no children). */
function treeLeaves(nodes, out = []) {
  for (const node of nodes || []) {
    if ((node.children || []).length === 0) out.push(node);
    else treeLeaves(node.children, out);
  }
  return out;
}

// ---------------------------------------------------------------------------
// unit
// ---------------------------------------------------------------------------

test('countDiffLines counts per file, including deleted files', () => {
  const diff = [
    'diff --git a/keep.txt b/keep.txt',
    '--- a/keep.txt',
    '+++ b/keep.txt',
    '@@ -1 +1,2 @@',
    '-old',
    '+new',
    '+extra',
    'diff --git a/gone.txt b/gone.txt',
    'deleted file mode 100644',
    '--- a/gone.txt',
    '+++ /dev/null',
    '@@ -1 +0,0 @@',
    '-bye',
  ].join('\n');
  const counts = panel.countDiffLines(diff);
  assert.deepEqual(counts.get('keep.txt'), { adds: 2, dels: 1 });
  assert.deepEqual(counts.get('gone.txt'), { adds: 0, dels: 1 });
});

test('resolveRepo prefers the explicit path over the workspace', () => {
  assert.equal(panel.resolveRepo({ path: '/repo' }, { workspace: '/ws' }), '/repo');
  assert.equal(panel.resolveRepo({}, { workspace: '/ws' }), '/ws');
  assert.throws(() => panel.resolveRepo({}, {}), /no repository/);
  assert.throws(() => panel.resolveRepo({ path: 'rel' }, {}), /absolute/);
});

test('the file tree groups directories and compacts single-child chains', () => {
  const tree = panel.buildFileTree([
    { path: 'crates/bsh/src/extension/e2e.rs', status: 'M', adds: 267, dels: 1 },
    { path: 'examples/extensions/change-history/lib/vcs.js', status: 'A', adds: 549, dels: 0 },
    { path: 'examples/extensions/change-history/index.js', status: 'A', adds: 26, dels: 0 },
  ]);
  assert.equal(tree.length, 2);
  assert.equal(tree[0].label, 'crates/bsh/src/extension');
  assert.equal(tree[0].children[0].label, 'e2e.rs');
  assert.equal(tree[0].children[0].detail, 'crates/bsh/src/extension');
  assert.equal(tree[0].children[0].badges[0].text, 'M');
  assert.equal(tree[0].children[0].badges[1].text, '+267');
  assert.equal(tree[0].children[0].action, 'file');
  const history = tree[1];
  assert.equal(history.label, 'examples/extensions/change-history');
  const labels = history.children.map((child) => child.label);
  assert.deepEqual(labels, ['lib', 'index.js']);
  assert.equal(history.children[0].children[0].label, 'vcs.js');
});

test('git decorations become HEAD, branch, remote and tag chips', () => {
  const badges = panel.gitBadges(' (HEAD -> main, origin/main, tag: v1.0)');
  assert.deepEqual(badges.map((badge) => badge.text), ['HEAD', 'main', 'origin/main', 'v1.0']);
  assert.equal(badges[0].icon, 'check');
  assert.equal(badges[1].icon, 'git-branch');
  assert.equal(badges[2].icon, 'globe');
  assert.equal(badges[3].icon, 'hash');
  assert.ok(badges.every((badge) => badge.style === 'chip'));
});

test('file icons and status tones follow the file class', () => {
  assert.equal(panel.fileIcon('a.js'), 'file-code');
  assert.equal(panel.fileIcon('a.rs'), 'file-code');
  assert.equal(panel.fileIcon('a.toml'), 'file-cog');
  assert.equal(panel.fileIcon('a.md'), 'file-text');
  assert.equal(panel.fileIcon('a.png'), 'file-image');
  assert.equal(panel.fileIcon('a.unknown'), 'file');
  assert.equal(panel.statusTone('A'), 'success');
  assert.equal(panel.statusTone('M'), 'warning');
  assert.equal(panel.statusTone('D'), 'error');
});

// ---------------------------------------------------------------------------
// git integration
// ---------------------------------------------------------------------------

test('git panel: card, commit, file diff and branch filter', async (t) => {
  if (!hasBinary('git')) return t.skip('git is not installed');
  const repo = scratchRepo('git');
  t.after(() => fs.rmSync(repo, { recursive: true, force: true }));
  gitInit(repo);
  const terminal = shellTerminal();
  const meta = { workspace: repo };

  // The live card: Git/Session tabs, message field, dropdown, history rail.
  let card = await panel.gitPanel(terminal, {}, meta);
  assertUiLimits(card);
  assert.equal(card.root.type, 'tabs');
  assert.deepEqual(card.root.tabs.map((tab) => tab.label), ['Git', 'Session']);
  const input = findNode(card.root, 'input');
  assert.equal(input.action, 'commit');
  assert.equal(input.submit_label, '');
  assert.match(input.placeholder, /Cmd\+Enter to commit on "main"/);
  assert.match(JSON.stringify(card), /No changes in the working copy/);
  const select = findNode(card.root, 'select');
  assert.equal(select.action, 'refresh');
  assert.ok(select.options.some((option) => option.value === 'main'));
  const timeline = findNode(card.root, 'timeline');
  assert.ok(timeline.items.some((item) => item.title === 'first commit'));
  assert.ok(timeline.items[0].badges.some((badge) => badge.text === 'HEAD'));

  // A working-tree change appears in the file tree with status and stats.
  fs.writeFileSync(path.join(repo, 'change.txt'), 'one\ntwo\n');
  card = await panel.gitPanel(terminal, {}, meta);
  assertUiLimits(card);
  const tree = findNode(card.root, 'tree');
  const leaves = treeLeaves(tree.nodes);
  assert.equal(leaves.length, 1);
  assert.equal(leaves[0].label, 'change.txt');
  assert.equal(leaves[0].badges[0].text, 'A');
  assert.equal(leaves[0].badges[1].text, '+2');
  assert.equal(leaves[0].action, 'file');
  // The stats row carries the totals and the file count badge.
  assert.match(JSON.stringify(card), /"text":"\+2"/);
  const countBadge = findNode(card.root, 'badge');
  assert.equal(countBadge.text, '1');
  assert.equal(countBadge.style, 'filled');

  // The file action shows that file's diff, by tree id or by list index.
  const fileCard = await panel.fileAction(terminal, { id: 'change.txt' }, meta);
  assertUiLimits(fileCard);
  const code = findNode(fileCard.root, 'code');
  assert.equal(code.language, 'diff');
  assert.match(code.value, /\+one/);
  const back = findNode(fileCard.root, 'button');
  assert.equal(back.action, 'refresh');
  const byIndex = await panel.fileAction(terminal, { index: 0 }, meta);
  assert.match(findNode(byIndex.root, 'code').value, /\+one/);
  await assert.rejects(panel.fileAction(terminal, { id: 'gone.txt' }, meta), /stale/);

  // Commit seals the change and refreshes the card.
  await assert.rejects(panel.commitAction(terminal, { value: '  ' }, meta), /enter a commit message/);
  card = await panel.commitAction(terminal, { value: 'panel: second commit' }, meta);
  assertUiLimits(card);
  assert.match(JSON.stringify(card), /No changes in the working copy/);
  assert.match(JSON.stringify(card), /panel: second commit/);
  await assert.rejects(
    panel.commitAction(terminal, { value: 'empty' }, meta),
    /nothing to commit/,
  );

  // The branch filter selects and persists across refreshes.
  card = await panel.gitPanel(terminal, { value: 'main' }, meta);
  assert.equal(findNode(card.root, 'select').value, 'main');
  card = await panel.gitPanel(terminal, {}, meta);
  assert.equal(findNode(card.root, 'select').value, 'main');
  // An unknown branch falls back to All branches rather than failing.
  card = await panel.gitPanel(terminal, { value: 'no-such-branch' }, meta);
  assert.equal(findNode(card.root, 'select').value, panel.ALL);
});

// ---------------------------------------------------------------------------
// jj integration
// ---------------------------------------------------------------------------

test('jj panel: card, commit and the working-copy badge', async (t) => {
  if (!hasBinary('jj')) return t.skip('jj is not installed');
  if (!hasBinary('git')) return t.skip('jj git backend needs git');
  const parent = scratchRepo('jj');
  const repo = path.join(parent, 'repo');
  t.after(() => fs.rmSync(parent, { recursive: true, force: true }));
  jjInit(repo);
  const terminal = shellTerminal();
  const meta = { workspace: repo };

  // The card detects jj; the working copy is the @ chip on the rail.
  fs.writeFileSync(path.join(repo, 'a.txt'), 'one\n');
  let card = await panel.gitPanel(terminal, {}, meta);
  assertUiLimits(card);
  const tree = findNode(card.root, 'tree');
  const leaves = treeLeaves(tree.nodes);
  assert.equal(leaves[0].label, 'a.txt');
  assert.equal(leaves[0].badges[1].text, '+1');
  const timeline = findNode(card.root, 'timeline');
  assert.ok(timeline.items[0].badges.some((badge) => badge.text === '@'));

  // Commit describes the working copy and starts a fresh change on top.
  card = await panel.commitAction(terminal, { value: 'panel: first change' }, meta);
  assertUiLimits(card);
  assert.match(JSON.stringify(card), /No changes in the working copy/);
  assert.match(JSON.stringify(card), /panel: first change/);

  // A bookmark becomes a branch filter option and a chip on the rail.
  const bookmark = spawnSync('jj', ['bookmark', 'create', 'main', '-r', '@-'], {
    cwd: repo,
    encoding: 'utf8',
  });
  assert.equal(bookmark.status, 0, bookmark.stderr);
  card = await panel.gitPanel(terminal, { value: 'main' }, meta);
  const select = findNode(card.root, 'select');
  assert.equal(select.value, 'main');
  assert.ok(select.options.some((option) => option.value === 'main'));
  const rail = findNode(card.root, 'timeline');
  assert.ok(rail.items.some((item) => item.badges.some((badge) => badge.text === 'main')));
});
