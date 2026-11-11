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
  - The launcher's popup takes the keyboard from an open kiosk, and when it closes the
    keyboard goes nowhere. The spike's `kiosk.focus` command, run after the launcher closes,
    was refused as unknown in the 0.1.13 sandbox, so instead kiosk takes the keyboard back
    whenever it loses it in the second after a launcher's request.
  - After the app crashes with kiosk open, a launcher's first request goes nowhere and the
    app shows an "unknown command" toast before the launcher opens kiosk. Only #726 can
    remove that.
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
- The only remembered state is unified versus side-by-side, in `preferences.toml` in the
  product's state directory beside the pending removals (rather than under the app's home,
  as first planned: kiosk already keeps state there), so the popup and a popped-out window
  agree. A preferences file kiosk can't read is reported, not overwritten.
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
- [x] Launcher mods: `mods/kiosk-<tab>`, one file each with its own `TAB`, which a test
      keeps in step with each other, with `TabId` and with kiosk's shared state. The
      sandbox installs them and the end-to-end suite drives them. To try one locally, link
      it as the README links `mods/kiosk`, and bind `mod.open:kiosk-diff` in `keys.json`.
- [ ] Release, install scripts and recipe ship the launchers. Held back until tabs leave
      `experimental_tabs`, so users don't get a mod for an unfinished tab. The installers
      swap `mods/kiosk` in whole and roll back on failure; that needs to cover every
      launcher folder too.
- [x] Upstream requests, linked under decisions.

### 3. Diff tab: core

- [x] Base resolution and the list of ranges (`kiosk_core::diff`).
- [x] Untracked files in the uncommitted range, through a copy of the checkout's index with
      everything added, so the checkout's own index is left alone. Adding writes blobs into
      the repository, as `git add` does; git prunes them in time.
- [x] Patch parsing, with renames, binary and mode changes. Paths, modes and object ids come
      from `--raw -z` records, so quoting never matters; only hunks come from the patch.
- [x] Lockfiles, binaries and huge files collapsed in the view (`e` or a click on the
      header expands one).
- [x] Syntax highlighting (`kiosk_core::diff::highlight`): whole files from each side's blob,
      as runs of `SyntaxTone`s per line. Scopes are matched against kiosk's own selector table
      rather than a TextMate theme, so the frontend's palette decides colours. Files over
      1 MiB, or with a line over 4 KiB (minified code), go plain, and so does any line whose
      runs don't cover it exactly.
- [x] Unified view (`kiosk_core::diff::tab` and `view::diff`): a sidebar of ranges and files,
      and the body as compact rows the mod draws with tone layers. Scrolling is by whole rows,
      owned by Rust as in the picker; the mod-owned smooth scrolling the rendering spike
      recommends is phase 4's.
- [x] Keys in `[keys.diff]`: scrolling, paging, top and bottom, next and previous file and
      range, collapsing, and quitting. The current file is a cursor of its own, so moving
      between files that all fit on screen still moves.
- [x] Empty states: not a repository, no folder (as for a remote machine's space), the
      default branch itself, detached HEAD, a range with no changes. A branch without a
      checkout is phase 5's way in.
- [x] Open the diff tab without search directories: opening on it skips the setup wizard
      and the no-search-directories screen, and the picker beside it finds nothing until
      they are set. (Without a config, `experimental_tabs` is off, so the wizard still runs.)
- [x] Help in the diff tab, listing its own keys, the tab keys, and quitting and help. Help
      lists commands in the order `commands!` declares them.

### 4. Diff tab: complete

- [x] Side-by-side, falling back to unified when narrow (the mod says whether 720 px of code
      fits), with `s` to toggle; the choice remembered.
- [x] Word-level highlighting within changed lines: removed lines pair with the added lines
      after them, and the words that differ between a pair are marked, unless less than 40%
      of the line is shared.
- [x] Live refresh (`kiosk_core::diff::watch`). `notify` watches the working tree's top level
      and each folder there git doesn't ignore (not `target/` or `node_modules/`, which on
      Linux would cost a watch per folder), folders made later at the top level, and git's
      directories for `HEAD`, the index, refs and `info/exclude`. The watch starts before the
      first diff is read, so no change slips between them. Reading files isn't a change.
      Once changes settle for 300 ms (or have gone on for 2 s), a worker checks them with
      `git check-ignore` and only if one matters reads the diff again, keeping the range, the
      file, the line at the top by its number, collapsed files and the column. A failed read
      is reported and the next change tries again; a failed watch is retried after 5 s, and
      then everything is read again, as what changed meanwhile isn't known. The index is
      copied by reading and writing it, as a macOS clone shows up as a change to it.
- [x] Open the file at the line in an editor: `o` types `[editor] command` (`{file}` and
      `{line}` filled in, the path quoted for the shell) into a new pane split off the focused
      pane of the checkout's space, then closes kiosk. Typing it into a shell rather than
      running it lets `$EDITOR` and aliases work, and leaves the shell when the editor quits.
      The line is the first changed line on screen, else the first line on screen, else the
      file's first change, by its number in the file now; a removed line opens at the line in
      its place. Deleted files, and a branch's files without its checkout, can't be opened.
- [x] Search within the diff: `/` opens a field in the header; typing finds the text in every
      line of the range (ignoring case unless the query has a capital), marks the matches and
      goes to the first at or below the top of the screen. `enter` and `shift+enter` (or the
      list keys) go round the matches, expanding a collapsed file a match is in, and `esc`
      ends the search. A new diff of the range, after a change or another range, is searched
      again.
- [x] Filter the files shown by their paths: `f` opens a field in the search's place that
      shows only the files whose paths match, by the picker's fuzzy matching, in the list and
      the body. `enter` keeps the filter and gives the keys back, `f` edits it again and `esc`
      drops it. File keys skip the files left out, search only looks in those shown, and a
      refreshed diff is filtered the same way.

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

### Shipping tabs

Once the diff tab is finished and polished:

- [ ] Drop `experimental_tabs`, document the tab and diff keys and `[editor]` in the
      README, and ship the launcher mods (phase 2's last item).
- [ ] Record the README preview again (`scripts/app-sandbox/run.sh record`): the
      repository and branch pickers, then the diff tab. Show the whole window scaled up, as
      on a smaller screen, rather than cropped, so it reads on small displays; and record once
      the repositories have been scanned and highlighting loaded, so it doesn't sit on
      loading.

### Later

- [ ] A merge preview: `git merge-tree --write-tree` against the base, with conflicts.
- [ ] Select lines and send review comments to the agent in that space.
