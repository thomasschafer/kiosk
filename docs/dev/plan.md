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

- Opening a tab from an app key. `mod.open` takes no props and mods can't import from another
  mod's folder (checked with `herdr-app check`: symlinks are refused too), so each extra tab
  gets a tiny launcher mod, such as `kiosk-diff`, a 1×1 frame-less popup that does its work
  in `init`. The spike (in the 0.1.13 sandbox) settled how:
  - Opening kiosk with `openOverlay({ mod: "kiosk", props })` gives it a generated overlay id,
    so `mod.open:kiosk` no longer toggles it and stacks a second copy. Instead the launcher
    sets `shared("kiosk.launch")` to the tab and runs `mod.open kiosk`; kiosk takes and clears
    it and passes it to `serve --tab`.
  - Nothing says whether an overlay is open, and `run()` doesn't report failure, so kiosk
    keeps `shared("kiosk.open")` itself. It persists, so after a crash it is stale. A
    launcher therefore sends `kiosk.tab:<tab>:<nonce>`, kiosk acknowledges in
    `shared("kiosk.ack")`, and a launcher that hears nothing in 150 ms clears the stale value
    and opens kiosk. Mod command arguments are dropped, so everything goes in the string.
  - The launcher's popup takes the keyboard from an open kiosk, so it closes itself and then
    runs `kiosk.focus`; commands are handled in order (seen, not documented).
  - Launching the tab that already shows closes kiosk, as pressing `mod.open`'s key again
    does; launching another switches to it. The picker decides, as it knows the tab.
  - Upstream: <https://github.com/herdrdev/herdr-app/issues/164> (`mod.open` props) and
    <https://github.com/herdrdev/herdr-app/issues/726> (listing overlays, `run()` failures).
    Either would remove most of this.
- Tabs are a closed enum in kiosk-core (`TabId`, declared once by a macro with each tab's
  name and title), and the session keeps each tab's state while another shows. `[keys.tabs]`
  is checked before every other section, in every tab, so a tab key always changes tab;
  `noop` there only unbinds the tab key. Other keys go to the tab that shows: the picker's
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
  app theme's colors. The app's built-in `Editor` highlights code but can't draw a diff
  (no line or word backgrounds, its own grammar, about 22 ms to replace its text), and
  `TextView.html` drops colours and whitespace.
- Drawing the diff. The rendering spike measured about 8–9 µs per element, almost all of it
  native, so one `div` per coloured span (2,000–4,600 elements for a side-by-side screen)
  runs at 20–30 fps. Sending rows is cheap: as compact arrays a screen is about 25 KB and
  parses in under 1 ms, against about 40 ms as plain JSON objects, which arrive over several
  socket reads.
  - Wire format: each half-row as its text plus runs of `[length, tone]`, tones indexing a
    palette sent once, with a bit for word-level emphasis. Rust merges adjacent runs, expands
    tabs and cuts lines at the visible width.
  - Drawing it today: "tone layers", one multi-line text per colour per side holding only
    that colour's characters (spaces elsewhere) in a monospace font, with line and word
    backgrounds as boxes behind. 60 fps in the sandbox. Wide characters and ligatures break
    the alignment, so Rust flags such lines and the mod draws them a span at a time.
  - One function in the mod draws a row from the wire format, so a styled-text element can
    replace tone layers when one exists:
    <https://github.com/herdrdev/herdr-app/issues/727>.
  - Scrolling: the mod owns the pixel offset over a buffer of about ±140 rows and asks for
    more near an edge; Rust owns the content, the cursor and jumps such as the next hunk,
    and sends a scroll target when one moves the view. Unlike the picker, where Rust
    scrolls.
  - Performance checks in the sandbox need a small window or the app's frame timings: its
    software rendering caps a full-size window at about 20 fps.
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
- Diagrams in the explain tab are Mermaid, rendered by `merman` (MIT/Apache-2.0) in-process
  to SVG the app shows with `image()`. It covers about 30 diagram types, takes theme colours
  and a font, has a mode that avoids SVG features the app's renderer lacks, renders each in
  2–5 ms, and is what Zed uses on the same UI framework. Its own text measurement is too
  narrow and clips labels, so kiosk measures with the real font. A slim build adds about
  10 MB; pin the version, as it is young. Diagrams that don't parse show as code. Graphviz
  laid out best but can't draw sequence diagrams and is an external program; D2 falls back
  to a serif font under the app's renderer; svgbob mangles code identifiers.
- The `Backend` trait stays: its generics are mostly contained in `spawn.rs`, the worktree
  backend and the pickers, the test mock relies on it, and the new tabs mostly talk to git.
  `NativeWorktrees` has no production implementation left and can go as a small cleanup,
  along with the mock backend's implementation and the tests of native worktree operations.

## Phases

### 1. Spikes

Throwaway work that answers a question; record the answer under decisions.

- [x] Launcher mod: in `scripts/app-sandbox`, does opening `kiosk` from another mod with
      props work, without a visible flash, and does switching tab in an open copy work?
- [x] Diagrams: which can kiosk render to SVG without a browser, and how well? Candidates
      are a pure-Rust Mermaid renderer, Graphviz and D2; agents can write any of them.
- [x] Rendering cost: can the mod draw a large, densely styled diff smoothly when it only
      receives the visible rows over the socket?

### 2. Tab framework

- [x] Tabs in kiosk-core, with the picker as the first, and protocol v4 carrying the tab bar.
      The diff tab says what it will show until phase 3 fills it in. Tabs are behind
      `experimental_tabs = true` in the config: without it kiosk shows only the picker and
      leaves the tab keys to the app. The setting and the `[keys.tabs]` and `[keys.diff]`
      sections are kept out of the README (`#[doc(hidden)]`, `KeyLayer::documented`). Drop
      the setting and document them once the diff tab is ready for users.
- [x] Keys to jump to a tab and to move between tabs, configurable in `[keys]`.
- [x] Panel size per tab.
- [x] Kiosk's side of launching: `serve --tab`, the shared state above, and `launch` on the
      wire.
- [ ] Launcher mods, installed by the install scripts and the recipe.
- [x] Upstream requests, linked under decisions.

### 3. Diff tab: core

- [ ] Base resolution and the list of ranges.
- [ ] Untracked files in the uncommitted range.
- [ ] Patch parsing, renames, binary and mode changes; lockfiles, binaries and huge files
      collapsed.
- [ ] Unified view with syntax highlighting.
- [ ] Empty states: not a repository, a remote machine's space, the default branch itself,
      detached HEAD, a branch without a checkout.
- [ ] Open the diff tab without search directories: today a missing config's setup wizard,
      or a config with no search directories, comes before any tab.
- [ ] Help in the diff tab: `ctrl+h` does nothing there yet, as help is the picker's.

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
