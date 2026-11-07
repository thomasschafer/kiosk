# Plan: tabs, a diff viewer and an explain view

This is the working plan for growing kiosk from a repository and branch picker into a set of
tabs. It is for whoever works on kiosk, not for users. Everything here can be changed:
decisions record what we chose and why, so they can be revisited when the reason no longer
holds. Tick items off in the PR that does them, and update the plan when it turns out to be
wrong.

## Goals

- Kiosk has tabs. The picker is one; others follow, so the tab system is general rather
  than built around two.
- Each tab can be opened directly with its own key bound in the Herdr app, jumped to with a
  kiosk key while kiosk is open, or reached by moving between tabs.
- A diff tab shows the current branch's changes with syntax highlighting and its commit
  history: uncommitted changes, each commit, and the whole branch against the default
  branch's merge-base.
- An explain tab shows an agent's explanation of a change: markdown, diagrams, and the
  important hunks annotated.

## Decisions

- Opening a tab from an app key. `mod.open` takes no props, mods can't import from another
  mod's folder (checked with `herdr-app check`: symlinks are refused too), so each extra tab
  gets a tiny launcher mod, such as `kiosk-diff`. When opened, it opens `kiosk` as an overlay
  with `props: { tab }` and closes itself, or, if kiosk is already open, sends the open copy
  a mod command to switch tab. We also ask upstream for `mod.open {name, props}`; if it is
  accepted the launchers can go.
- Tabs are a closed enum in kiosk-core, each owning its state, so the active tab and its
  state are one typed value. Keymap resolution depends on the active tab: the picker's
  search takes typed characters, while the diff tab can use single-letter keys.
- Panel size is per tab. The overlay is already a frame-less 80% dialog that kiosk draws its
  own panel inside, so the diff tab can grow to fill it without reopening.
- The diff base is the merge-base with the remote's default branch (`origin/HEAD`), falling
  back to the local default branch, and the view says which base it used. Local `main` is
  often stale, and a branch made from or rebased onto `origin/main` would otherwise show
  other people's commits as its own.
- The default range is everything since the merge-base, including uncommitted and untracked
  changes: what the PR would be if you committed now. Narrower ranges are uncommitted
  changes alone and each commit.
- Diffs come from the git CLI, as the rest of kiosk does, parsed in Rust. Rust keeps the
  whole diff and sends only the visible rows. Syntax highlighting uses syntect over whole
  files (not hunks, which break multi-line strings and comments), with scopes mapped to the
  app theme's colors. The app's built-in `Editor` highlights code but can't draw a diff.
- Diff lines are addressable by file, side and line from the start, for the explain tab and
  for sending review comments to agents later.
- Live refresh watches files from Rust, filtered by `.gitignore`, plus `.git`'s HEAD, refs
  and index, debounced, keeping the place on the same hunk. The mod's `watchFolder` sees
  ignored folders on macOS, so a build would refresh constantly.
- The only remembered state is unified versus side-by-side, written by Rust to
  `data/kiosk/state.json` under the app's home so the popup and a popped-out window agree.
- Explanations reference hunks rather than quoting them, pinned to a snapshot so they can't
  drift from what they describe. `herdr-app-kiosk explain snapshot` records the base and
  head commits and, if there are uncommitted changes, a commit of the working tree
  (untracked files included, through a temporary index), kept alive under
  `refs/kiosk/explain/<id>`. The agent reads that snapshot's diff and writes markdown naming
  the snapshot. `herdr-app-kiosk explain show <file>` checks every reference resolves, and
  refuses with an error the agent can act on if not. When the branch has since moved, the
  tab says so and offers the current diff.
- The `Backend` trait stays: its generics are mostly contained in `spawn.rs`, the worktree
  backend and the pickers, the test mock relies on it, and the new tabs mostly talk to git.
  `NativeWorktrees` has no production implementation left and can go as a small cleanup,
  along with the mock backend's implementation and the tests of native worktree operations.

## Phases

### 1. Spikes

Throwaway work that answers a question; record the answer under decisions.

- [ ] Launcher mod: in `scripts/app-sandbox`, does opening `kiosk` from another mod with
      props work, without a visible flash, and does switching tab in an open copy work?
- [ ] Diagrams: which can kiosk render to SVG without a browser, and how well? Candidates
      are a pure-Rust Mermaid renderer, Graphviz and D2; agents can write any of them.
- [ ] Rendering cost: can the mod draw a large, densely styled diff smoothly when it only
      receives the visible rows over the socket?

### 2. Tab framework

- [ ] Tabs in kiosk-core, with the picker as the first, and protocol v4 carrying the tab bar.
- [ ] Keys to jump to a tab and to move between tabs, configurable in `[keys]`.
- [ ] Panel size per tab.
- [ ] Launcher mods, installed by the install scripts and the recipe.
- [ ] Upstream request for `mod.open {name, props}`.

### 3. Diff tab: core

- [ ] Base resolution and the list of ranges.
- [ ] Untracked files in the uncommitted range.
- [ ] Patch parsing, renames, binary and mode changes; lockfiles, binaries and huge files
      collapsed.
- [ ] Unified view with syntax highlighting.
- [ ] Empty states: not a repository, a remote machine's space, the default branch itself,
      detached HEAD, a branch without a checkout.

### 4. Diff tab: complete

- [ ] Side-by-side, falling back to unified when narrow, with a key to toggle; the choice
      remembered.
- [ ] Word-level highlighting within changed lines.
- [ ] Live refresh.
- [ ] Open the file at the line in an editor, through a configurable command.
- [ ] Search within the diff, and filter files.

### 5. Ways in

- [ ] A key in the repository list and the branch list that opens the diff tab for the
      selection.
- [ ] Pop out the current tab into a window, keeping its place.

### 6. Explain tab

- [ ] `herdr-app-kiosk explain snapshot` and `explain show`, with validation.
- [ ] The document format, and rendering markdown, diagrams and annotated hunks with the
      diff tab's renderer.
- [ ] Opens as a window by default.
- [ ] Update the explain-diff skill (in the dotfiles repository) to write for it.

### Later

- [ ] A merge preview: `git merge-tree --write-tree` against the base, with conflicts.
- [ ] Select lines and send review comments to the agent in that space.
