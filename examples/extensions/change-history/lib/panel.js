'use strict';
/**
 * The Git panel: a live declarative card over the change-history engine,
 * shaped after the reference Git view — a Changes section with the commit
 * message field, total stats and a file tree with per-file status and line
 * counts, and a Graph section with a branch dropdown and a commit rail with
 * HEAD/branch chips.
 *
 * The live card is a ctx.panels registration named after the panel; three
 * tools serve its actions, one per action id:
 *   refresh — the branch select's and the footer's action: the card
 *   commit  — the message field's action: seal the working copy
 *   file    — a change row's action: that file's diff, with a way back
 *
 * The repository is the call's workspace (the host hands it to execute as
 * the second argument); an explicit path argument overrides it.
 */

const fs = require('node:fs');
const path = require('node:path');

const vcs = require('./vcs.js');

/** The branch filter's "everything" option. */
const ALL = 'all';
/** Card budgets, inside the host's UI limits (1000 nodes, 16 KiB a leaf). */
const MAX_FILES = 200;
const MAX_COMMITS = 30;
const MAX_DIFF_CHARS = 16000;

/** The selected branch per repository, kept across live refreshes. */
const branchState = new Map();

function fail(message) {
  throw new vcs.VcsError(message);
}

/** The repository a panel call acts on: explicit path, else the workspace. */
function resolveRepo(args, meta) {
  const repo = (args && args.path) || (meta && meta.workspace);
  if (typeof repo !== 'string' || repo.length === 0) {
    fail('no repository: open a workspace, or pass path (the absolute repository path)');
  }
  if (!path.isAbsolute(repo)) {
    fail('path must be absolute so the capability scope and the command agree');
  }
  return repo;
}

/** The host catalog icon a file class draws with. */
function fileIcon(filePath) {
  const ext = filePath.split('.').pop().toLowerCase();
  if (['js', 'mjs', 'cjs', 'ts', 'tsx', 'jsx', 'py', 'rb', 'go', 'rs', 'c', 'cpp', 'h', 'sh', 'java'].includes(ext)) return 'file-code';
  if (['md', 'txt', 'rst'].includes(ext)) return 'file-text';
  if (['toml', 'yaml', 'yml', 'ini', 'cfg', 'conf'].includes(ext)) return 'file-cog';
  if (ext === 'json') return 'braces';
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'svg', 'ico'].includes(ext)) return 'file-image';
  if (['zip', 'tar', 'gz', 'tgz', 'rar', '7z'].includes(ext)) return 'file-archive';
  if (['csv', 'xlsx', 'parquet'].includes(ext)) return 'file-spreadsheet';
  return 'file';
}

/** The semantic tone a change status letter carries. */
function statusTone(status) {
  if (status === 'A') return 'success';
  if (status === 'D') return 'error';
  if (status === 'R') return 'accent';
  if (status === 'M') return 'warning';
  return 'muted';
}

/** Count + and - lines per file in a git-format diff. */
function countDiffLines(text) {
  const counts = new Map();
  let current = null;
  for (const line of text.split('\n')) {
    if (line.startsWith('diff --git ')) {
      current = null;
      continue;
    }
    if (line.startsWith('+++ b/')) {
      current = line.slice(6);
      if (!counts.has(current)) counts.set(current, { adds: 0, dels: 0 });
      continue;
    }
    if (line.startsWith('--- a/') && current === null) {
      // A deleted file's +++ is /dev/null; the --- line names it.
      current = line.slice(6);
      if (!counts.has(current)) counts.set(current, { adds: 0, dels: 0 });
      continue;
    }
    // The remaining header lines (+++ /dev/null, --- a/…) are not content.
    if (line.startsWith('+++') || line.startsWith('---')) continue;
    if (current === null) continue;
    if (line.startsWith('+')) counts.get(current).adds += 1;
    else if (line.startsWith('-')) counts.get(current).dels += 1;
  }
  return counts;
}

/** The working copy's changed files with status and exact line counts. */
async function collectChanges(terminal, repo, backend, platform) {
  const files = [];
  if (backend === 'jj') {
    const summary = await vcs.run(terminal, repo, 'jj', ['--color=never', 'diff', '--summary'], platform);
    const diff = await vcs.run(terminal, repo, 'jj', ['--color=never', 'diff', '--git'], platform);
    const counts = countDiffLines(diff.stdout.text);
    for (const line of summary.stdout.text.split('\n')) {
      if (line.length < 3) continue;
      const status = line[0];
      const filePath = line.slice(2);
      const count = counts.get(filePath) || { adds: 0, dels: 0 };
      files.push({ path: filePath, status, adds: count.adds, dels: count.dels });
    }
  } else {
    const git = (extra) =>
      vcs.run(terminal, repo, 'git', ['--no-pager', '-c', 'color.ui=false', ...extra], platform);
    const status = await git(['status', '--porcelain=v1']);
    const numstat = await git(['diff', 'HEAD', '--numstat']);
    const counts = new Map();
    for (const line of numstat.stdout.text.split('\n')) {
      if (line.length === 0) continue;
      const [adds, dels, ...rest] = line.split('\t');
      counts.set(rest.join('\t'), {
        adds: adds === '-' ? null : Number(adds),
        dels: dels === '-' ? null : Number(dels),
      });
    }
    for (const line of status.stdout.text.split('\n')) {
      if (line.length < 4) continue;
      const xy = line.slice(0, 2);
      const filePath = line.slice(3);
      const untracked = xy === '??';
      const statusLetter = untracked ? 'A' : xy.includes('D') ? 'D' : xy.includes('R') ? 'R' : xy.includes('A') ? 'A' : 'M';
      let count = counts.get(filePath) || { adds: null, dels: null };
      if (untracked) {
        // numstat skips untracked files; diff --no-index counts them (exit 1
        // means "the files differ", its success case).
        const added = await vcs.run(
          terminal, repo, 'git',
          ['--no-pager', '-c', 'color.ui=false', 'diff', '--no-index', '--numstat', '--', '/dev/null', filePath],
          platform,
          { okCodes: [0, 1] },
        );
        const fields = added.stdout.text.trim().split('\t');
        count = { adds: Number(fields[0]) || 0, dels: 0 };
      }
      files.push({
        path: filePath,
        status: statusLetter,
        adds: count.adds,
        dels: count.dels,
        untracked,
      });
    }
  }
  let adds = 0;
  let dels = 0;
  for (const file of files) {
    adds += file.adds || 0;
    dels += file.dels || 0;
  }
  return { files, totals: { adds, dels } };
}

/** The head label the message field and footer name. */
async function headInfo(terminal, repo, backend, platform) {
  if (backend === 'jj') {
    const out = await vcs.run(
      terminal, repo, 'jj',
      ['--color=never', 'log', '--no-graph', '-r', '@', '-n', '1', '-T',
       'change_id.short() ++ "\\t" ++ if(description, description.first_line(), "(no description)")'],
      platform,
    );
    const [id, description] = out.stdout.text.trim().split('\t');
    return { id, description, label: '@ ' + id };
  }
  const git = (extra) =>
    vcs.run(terminal, repo, 'git', ['--no-pager', '-c', 'color.ui=false', ...extra], platform);
  let branch = null;
  try {
    const out = await git(['symbolic-ref', '--short', '-q', 'HEAD']);
    branch = out.stdout.text.trim() || null;
  } catch {
    branch = null;
  }
  const out = await git(['log', '-1', '--format=%h%x1f%s']);
  const [short, subject] = out.stdout.text.trim().split('\x1f');
  return { id: short, description: subject, label: branch || 'Detached HEAD' };
}

/** Turn a git decoration string into branch/HEAD/remote chips. */
function gitBadges(decoration) {
  const badges = [];
  const inner = (decoration || '').replace(/^\s*\(/, '').replace(/\)\s*$/, '');
  for (const part of inner.split(',')) {
    const label = part.trim();
    if (label.length === 0) continue;
    if (label.startsWith('HEAD -> ')) {
      badges.push({ text: 'HEAD', tone: 'accent', icon: 'check', style: 'chip' });
      badges.push({ text: label.slice(8), tone: 'accent', icon: 'git-branch', style: 'chip' });
    } else if (label === 'HEAD') {
      badges.push({ text: 'HEAD', tone: 'accent', icon: 'check', style: 'chip' });
    } else if (label.startsWith('tag: ')) {
      badges.push({ text: label.slice(5), tone: 'neutral', icon: 'hash', style: 'chip' });
    } else if (label.includes('/')) {
      badges.push({ text: label, tone: 'neutral', icon: 'globe', style: 'chip' });
    } else {
      badges.push({ text: label, tone: 'accent', icon: 'git-branch', style: 'chip' });
    }
  }
  return badges;
}

/** The branch list and the commit graph for the selected filter. */
async function collectGraph(terminal, repo, backend, branch, platform, headId) {
  const branches = [];
  const commits = [];
  if (backend === 'jj') {
    const names = await vcs.run(terminal, repo, 'jj', ['bookmark', 'list', '-T', 'name ++ "\\n"'], platform);
    for (const line of names.stdout.text.split('\n')) {
      if (line.trim().length > 0) branches.push(line.trim());
    }
    const selected = branches.includes(branch) ? branch : ALL;
    const revset = selected === ALL ? 'all()' : 'ancestors("' + selected + '", ' + MAX_COMMITS + ')';
    const out = await vcs.run(
      terminal, repo, 'jj',
      ['--color=never', 'log', '--no-graph', '-r', revset, '-n', String(MAX_COMMITS), '-T',
       'change_id.short() ++ "\\t" ++ commit_id.short() ++ "\\t" ++ if(description, description.first_line(), "(no description)") ++ "\\t" ++ bookmarks ++ "\\n"'],
      platform,
    );
    for (const line of out.stdout.text.split('\n')) {
      if (line.length === 0) continue;
      const [changeId, , description, bookmarks] = line.split('\t');
      const badges = [];
      if (changeId === headId) badges.push({ text: '@', tone: 'accent', icon: 'git-commit', style: 'chip' });
      for (const name of (bookmarks || '').split(',')) {
        if (name.trim().length > 0) {
          badges.push({ text: name.trim(), tone: 'accent', icon: 'git-branch', style: 'chip' });
        }
      }
      commits.push({ id: changeId, description, badges });
    }
    return { branches, commits, selected };
  }
  const git = (extra) =>
    vcs.run(terminal, repo, 'git', ['--no-pager', '-c', 'color.ui=false', ...extra], platform);
  const names = await git(['branch', '--format=%(refname:short)']);
  for (const line of names.stdout.text.split('\n')) {
    const name = line.trim();
    if (name.length > 0 && name !== '(no branch)') branches.push(name);
  }
  const selected = branches.includes(branch) ? branch : ALL;
  const extra = ['log', '-n', String(MAX_COMMITS), '--format=%h%x1f%H%x1f%s%x1f%d'];
  if (selected !== ALL) extra.push(selected);
  const out = await git(extra);
  for (const line of out.stdout.text.split('\n')) {
    if (line.length === 0) continue;
    const [short, , subject, decoration] = line.split('\x1f');
    commits.push({ id: short, description: subject, badges: gitBadges(decoration) });
  }
  return { branches, commits, selected };
}

/** Build the file tree: directories grouped, single-child chains compacted. */
function buildFileTree(files) {
  const root = { dirs: new Map(), files: [] };
  for (const file of files.slice(0, MAX_FILES)) {
    const parts = file.path.split('/');
    let node = root;
    for (const part of parts.slice(0, -1)) {
      if (!node.dirs.has(part)) node.dirs.set(part, { dirs: new Map(), files: [] });
      node = node.dirs.get(part);
    }
    node.files.push(file);
  }
  const fileNode = (file) => ({
    id: file.path,
    label: file.path.split('/').pop(),
    detail: file.path.includes('/') ? file.path.slice(0, file.path.lastIndexOf('/')) : '',
    icon: fileIcon(file.path),
    badges: [
      { text: file.status, tone: statusTone(file.status), style: 'plain' },
      { text: '+' + (file.adds || 0), tone: 'success', style: 'plain' },
      { text: '\u2212' + (file.dels || 0), tone: 'error', style: 'plain' },
    ],
    action: 'file',
  });
  const render = (node, prefix) => {
    const out = [];
    const dirNames = [...node.dirs.keys()].sort();
    for (const name of dirNames) {
      let child = node.dirs.get(name);
      let label = name;
      // Compact a chain of single-child directories, as the reference does.
      while (child.files.length === 0 && child.dirs.size === 1) {
        const [nextName] = [...child.dirs.keys()];
        label += '/' + nextName;
        child = child.dirs.get(nextName);
      }
      out.push({
        id: prefix + label,
        label,
        icon: 'folder',
        open: true,
        children: render(child, prefix + label + '/'),
      });
    }
    for (const file of node.files.sort((a, b) => a.path.localeCompare(b.path))) {
      out.push(fileNode(file));
    }
    return out;
  };
  return render(root, '');
}

/** Assemble the UiDocument the host renders. */
function buildCard(state) {
  const { backend, head, files, totals, branches, commits, selected } = state;
  const dirty = files.length > 0;
  const changesChildren = [
    {
      type: 'input',
      label: '',
      value: '',
      placeholder: 'Message (Cmd+Enter to commit on "' + head.label + '")',
      action: 'commit',
      submit_label: '',
    },
    {
      type: 'row',
      children: [
        { type: 'text', text: 'Changes' },
        { type: 'text', text: '+' + totals.adds, tone: 'success' },
        { type: 'text', text: '\u2212' + totals.dels, tone: 'error' },
        { type: 'fill' },
        { type: 'badge', text: String(files.length), tone: 'neutral', style: 'filled' },
      ],
    },
  ];
  if (files.length === 0) {
    changesChildren.push({ type: 'text', text: 'No changes in the working copy', tone: 'muted' });
  } else {
    changesChildren.push({ type: 'tree', nodes: buildFileTree(files) });
  }
  const gitTab = {
    type: 'column',
    children: [
      { type: 'section', title: 'Changes', open: true, children: changesChildren },
      {
        type: 'section',
        title: 'Graph',
        open: true,
        children: [
          {
            type: 'select',
            label: '',
            value: selected,
            options: [{ value: ALL, label: 'All branches' }].concat(
              branches.map((name) => ({ value: name, label: name })),
            ),
            action: 'refresh',
          },
          {
            type: 'timeline',
            items: commits.map((commit) => ({
              title: commit.description || '(no description)',
              detail: commit.id,
              badges: commit.badges,
            })),
          },
        ],
      },
      {
        type: 'row',
        children: [
          { type: 'badge', text: head.label + (dirty ? '*' : ''), tone: 'muted', icon: 'git-branch', style: 'plain' },
          { type: 'fill' },
          { type: 'button', label: head.label, icon: 'refresh-cw', style: 'ghost', action: 'refresh' },
        ],
      },
    ],
  };
  return {
    schema_version: '1.0',
    root: {
      type: 'tabs',
      tabs: [
        { id: 'git', label: 'Git', content: gitTab },
        {
          id: 'session',
          label: 'Session',
          content: {
            type: 'column',
            children: [
              { type: 'text', text: 'Session change tracking is not available to extensions yet.', tone: 'muted' },
              { type: 'text', text: 'The Git tab shows the working copy and the commit history.', tone: 'faint' },
            ],
          },
        },
      ],
    },
  };
}

/** Read everything one card shows. */
async function collect(terminal, repo, backend, branch, platform) {
  const head = await headInfo(terminal, repo, backend, platform);
  const changes = await collectChanges(terminal, repo, backend, platform);
  const graph = await collectGraph(terminal, repo, backend, branch, platform, head.id);
  return {
    backend,
    branch,
    head,
    files: changes.files,
    totals: changes.totals,
    branches: graph.branches,
    commits: graph.commits,
    selected: graph.selected,
  };
}

/** The refresh tool and the panel render: the live card. */
async function gitPanel(terminal, rawArgs, meta, deps = {}) {
  const platform = deps.platform || process.platform;
  const exists = deps.exists || fs.existsSync;
  const args = rawArgs && typeof rawArgs === 'object' ? rawArgs : {};
  const repo = resolveRepo(args, meta);
  if (!exists(repo)) fail('path does not exist: ' + repo);
  const backend = vcs.detectBackend(repo, args.backend || 'auto', exists);
  let branch = branchState.get(repo) || ALL;
  if (typeof args.value === 'string') {
    branch = args.value;
    branchState.set(repo, branch);
  }
  const state = await collect(terminal, repo, backend, branch, platform);
  return buildCard(state);
}

/** The commit tool: seal the working copy, then show the fresh card. */
async function commitAction(terminal, rawArgs, meta, deps = {}) {
  const platform = deps.platform || process.platform;
  const exists = deps.exists || fs.existsSync;
  const args = rawArgs && typeof rawArgs === 'object' ? rawArgs : {};
  const repo = resolveRepo(args, meta);
  if (!exists(repo)) fail('path does not exist: ' + repo);
  const backend = vcs.detectBackend(repo, args.backend || 'auto', exists);
  const message = typeof args.value === 'string' ? args.value.trim() : '';
  if (message.length === 0) fail('enter a commit message');
  const changes = await collectChanges(terminal, repo, backend, platform);
  if (changes.files.length === 0) fail('nothing to commit: the working copy is clean');
  if (backend === 'jj') {
    await vcs.run(terminal, repo, 'jj', ['--color=never', 'commit', '-m', message], platform);
  } else {
    const git = (extra) =>
      vcs.run(terminal, repo, 'git', ['--no-pager', '-c', 'color.ui=false', ...extra], platform);
    await git(['add', '-A']);
    await git(['commit', '-m', message]);
  }
  return gitPanel(terminal, { path: repo }, meta, deps);
}

/** The file tool: one change row's diff, with a way back to the card. */
async function fileAction(terminal, rawArgs, meta, deps = {}) {
  const platform = deps.platform || process.platform;
  const exists = deps.exists || fs.existsSync;
  const args = rawArgs && typeof rawArgs === 'object' ? rawArgs : {};
  const repo = resolveRepo(args, meta);
  if (!exists(repo)) fail('path does not exist: ' + repo);
  const backend = vcs.detectBackend(repo, args.backend || 'auto', exists);
  const changes = await collectChanges(terminal, repo, backend, platform);
  // A tree row carries its path as id; a flat list row carries its index.
  let file = null;
  if (typeof args.id === 'string') {
    file = changes.files.find((candidate) => candidate.path === args.id) || null;
  } else if (Number.isInteger(args.index)) {
    file = changes.files[args.index] || null;
  } else {
    fail('file needs a row id or index');
  }
  if (!file) fail('that file row is stale; refresh the panel');
  let diff;
  if (backend === 'jj') {
    diff = await vcs.run(terminal, repo, 'jj', ['--color=never', 'diff', '--git', file.path], platform);
  } else if (file.untracked) {
    diff = await vcs.run(
      terminal, repo, 'git',
      ['--no-pager', '-c', 'color.ui=false', 'diff', '--no-index', '--', '/dev/null', file.path],
      platform,
      { okCodes: [0, 1] },
    );
  } else {
    diff = await vcs.run(
      terminal, repo, 'git',
      ['--no-pager', '-c', 'color.ui=false', 'diff', 'HEAD', '--', file.path],
      platform,
    );
  }
  return {
    schema_version: '1.0',
    root: {
      type: 'column',
      children: [
        {
          type: 'row',
          children: [
            { type: 'button', label: 'Back', action: 'refresh', style: 'ghost' },
            { type: 'heading', text: file.path, level: 3 },
          ],
        },
        { type: 'code', language: 'diff', value: diff.stdout.text.slice(0, MAX_DIFF_CHARS) },
      ],
    },
  };
}

module.exports = {
  ALL,
  MAX_COMMITS,
  MAX_DIFF_CHARS,
  MAX_FILES,
  buildCard,
  buildFileTree,
  collectChanges,
  commitAction,
  countDiffLines,
  fileAction,
  fileIcon,
  gitBadges,
  gitPanel,
  resolveRepo,
  statusTone,
};
