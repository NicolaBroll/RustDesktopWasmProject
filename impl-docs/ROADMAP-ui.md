# UI and UX: step-by-step roadmap

Fixes from the UI review of 2026-10-30 (desktop app on Xvfb, `target/demo-repo`, English and
Italian, window from 1200x760 down to 760x520). The rules of
[ROADMAP-quality.md](ROADMAP-quality.md) apply here too: each step is small, closes with a green
`cargo xtask ci`, leaves the project working and changes only the behavior it names. The **Goal**
field can be used as is as the goal of a session; **Done when** says how to verify it.

Rules that apply to every step:

- Start from the latest `origin/dev` (see `AGENTS.md`, "Branches").
- Command ids and the JSON of the results do not change; visible texts change only through the
  keys in `crates/i18n/locales/{it,en}/`, in both languages.
- Check the result on Xvfb (`make desktop-start REPO=target/demo-repo`, `xdotool windowsize` to
  resize) at 1200x760 and at 760x520, in English and with `LANG=it_IT.UTF-8`.
- Update the step's checkbox in this file when it is finished.

## Phase U0 — Behavior

### U00 — The open diff stays during the automatic refresh
- [x] **Agent:** `egui-ui`
- **Goal:** when the window gets focus back (also while it is resized, on Windows and GNOME) the
  automatic refresh re-reads the status, `follow_diff` calls `DiffPane::reload` and that drops
  the loaded diff (`answer = None`) and the line selection: the view flashes "Loading the
  diff…", loses the scroll, the `+n -m` counters and the "Stage selected lines" bar. `reload`
  keeps the old diff on screen until the new one arrives; if the new one is the same, the
  selection stays, otherwise it is cleared.
- **Done when:** unit test on `DiffPane` (reload keeps the answer, same diff keeps the
  selection, different diff clears it); on Xvfb focus lost and regained with an unstaged diff
  open shows no loading state; CI green.
- **Done:** `DiffState::reload` keeps a loaded diff (`reloading`) and `receive` keeps document
  and selection when the same diff comes back; after an error it still shows the loading state.
  Stage hunk / selected lines wait while `is_reloading()` (the engine refuses a stale diff
  anyway). The status bar shows a read only after 400 ms (`Job::started`), a write at once: the
  refresh on focus no longer flashes "repo.status in progress…".

## Phase U1 — Layout and resizing

### U10 — Side panels follow the window
- [x] **Agent:** `egui-ui`
- **Goal:** the side panels' `max_size` (a third of the window) clamps their width when the
  window shrinks and egui stores the clamped width: when the window grows again they stay
  narrow. At small widths the center (graph, diff) gets less room than the two panels together
  (~280 px of 760). The panels keep the width chosen by the user and are only clamped while
  drawing, and the center keeps at least ~40% of the window: the panels shrink first, down to
  their minimum.
- **Done when:** 1200 → 900 → 1200 gives back the starting widths; at 760x520 the center is
  wider than each side panel; CI green.
- **Done:** `crates/ui/src/side_panel.rs` keeps the width chosen by the user (persisted beside
  egui's panel state) and before every frame writes it, clamped, into the `PanelState` egui
  draws from; a drawn width other than the one asked is a drag and becomes the new choice. Each
  panel gets at most 30% of the window (`max_width`, the center keeps 40%), never less than its
  minimum. On Xvfb 1200 → 760 → 1200 gives back 320 px, a dragged width survives the same round.

### U11 — Headers without overlaps
- [x] **Agent:** `egui-ui`
- **Goal:** in a narrow center the diff header draws "in commit c13e0f9" / "unstaged" under the
  `+n -m` counters, the "Stage hunk" button covers the `@@ … @@` text and the split-view hint is
  cut at the right edge. The counters and the view toggles keep their place on the right and the
  description is cut with an ellipsis before them; the hunk header text is clipped before its
  button; the hint wraps.
- **Done when:** at 760x520 no text overlaps another in the diff of a commit, of an unstaged
  file and in the split view; CI green.
- **Done:** the diff bar lays out the toggles and the counters first, from the right; the path
  is truncated in the room left and the description ("in commit …", "unstaged") is shown whole
  or not at all (it is also in the path's tooltip). "Stage hunk" sits on an opaque strip of the
  hunk color, so the header text stops before it; the split-view hint wraps.

### U12 — Stage panel: titles before buttons, no jump with a dialog open
- [x] **Agent:** `egui-ui`
- **Goal:** "Unstaged (2)" becomes "Unstage…" ("Non in sta…" in Italian already at the default
  width) because the buttons on the same row take the room first, and "Uncommitted changes" is
  cut beside "Stash…". The section title keeps its full text; when the row is too narrow the
  buttons go on the next line. With a dialog open the commit box moves ~13 px down and its hint
  disappears: the layout is the same whether a dialog is open or not.
- **Done when:** at 760x520, in both languages, the titles of the stage panel are whole; a
  screenshot with the stash dialog open and one without show the commit box at the same place;
  CI green.
- **Done:** `title_row` in `view/stage_view.rs` measures title and buttons: one row when the
  title fits whole, otherwise the title (wrapped) and the buttons on the row under it; used by
  the panel heading and by both sections. The list height is computed after the header
  (`Space { height, below }`) and the scroll area may go under egui's 64 px, so the lists never
  cover the commit box (at 760x520 the unstaged list is a short scrolled strip). With a dialog
  open the commit hint row is kept empty: the box moves 1 px instead of 13.

## Phase U2 — Text

### U20 — A type scale
- [x] **Agent:** `egui-ui`
- **Goal:** the font sizes are written by hand in each view (11, 11.5, 12, 12.5, 13, 13.5) and
  `TextStyle::Small` is egui's default 9 pt, used for the status bar, the branch label at the top
  left, the stash sub-line, the commit hint and counter: too small next to the body text. The
  theme defines the sizes once (body, small at least 11 px, monospace, heading) and the views
  use them.
- **Done when:** no `FontId::proportional(<number>)` / `monospace(<number>)` left outside the
  theme; the small texts are readable on the Xvfb screenshot; CI green.
- **Done:** `theme::size` (small 11, body 13, mono 12.5, heading 18) with `theme::body()`,
  `small()`, `mono()`; `theme::install_text_styles` sets egui's text styles on it at startup
  (`Small` was 9 pt). Commit rows, diff, blame, details, stage list and toolbar labels use the
  scale; the only literal sizes left are the icon-font checks in the `icons` tests.

### U21 — Wording and repeated header
- [x] **Agent:** `egui-ui`
- **Goal:** Italian "Unstage di tutto" becomes "Togli tutto dallo stage" and the hint "Scrivere
  il messaggio del commit" an imperative ("Scrivi …"); "Ctrl+Comma" is shown as "Ctrl+," in both
  languages, as "Ctrl+Enter" is shown with its key. The diff of one file shows its name twice
  (bar "README.md in commit …" and the file row "README.md modified +2 -0"): with a single file
  the bar does not repeat what the file row says.
- **Done when:** the texts above changed in both locales; i18n tests green; screenshot of a
  one-file diff; CI green.
- **Done:** Italian "Togli tutto dallo stage", "Scrivi il messaggio del commit" (and "Scrivi il
  messaggio" in the merge dialog). The File menu writes the settings shortcut with the key's
  symbol (`shortcut_text` in `view/title_bar.rs`: "Ctrl+,"; egui's symbols on macOS). In the diff
  of a single file that is not renamed the file row shows only the kind of change
  (`diff::file_kind`): path and counters are in the bar.

## Phase U3 — Second review: bugs and blocks

Fixes from the second UI review of 2026-10-30 (same setup as above, screenshots
`target/ux-*.png`).

### U30 — Stage panel at 760x520 in Italian
- [x] **Agent:** `egui-ui`
- **Goal:** at 760x520 in Italian "Non in stage (2)" overlaps "Messaggio di commit", the buttons
  "Scarta tutto…" / "Stage di tutto" cover the subject field and the unstaged list has zero
  height: the files cannot be seen nor staged. The commit hint (4 lines in Italian) eats the
  room of the lists. Also the commit box moves while typing (y 563 → 579 → 550) because the hint
  changes height. The hint has a fixed height (one line, cut, whole text in the tooltip of the
  button) and the lists keep at least one visible row.
- **Done when:** at 760x520 in both languages no overlaps and at least one file row of each
  list visible; the commit box does not move between empty message, typing and after a commit;
  CI green.
- **Done:** the commit box takes what the part above leaves, measured in the previous frame
  (`StageLayout` in `view/stage_view.rs`: headers plus one row per list); in a low window the
  body loses rows (down to one). The hint under "Commit" is one line cut with "…" (whole text in
  the button's tooltip) and keeps its row also when empty, so the box does not move.

### U31 — Esc with a menu open
- [x] **Agent:** `egui-ui`
- **Goal:** with a diff open, Esc that closes the File/Help menu (or a context menu) also
  closes the diff (`lib.rs` keyboard handling does not check open popups). Esc closes only the
  topmost thing.
- **Done when:** unit or Xvfb check: menu open over a diff, Esc → menu closed, diff still open;
  CI green.
- **Done:** the frame notes whether a menu or popup is open before drawing; then the tab takes
  no keys, so Esc closes only the menu (`keys_tests.rs`, checked on Xvfb).

### U32 — Readable "local changes" refusals
- [x] **Agent:** `egui-ui`
- **Goal:** merge, cherry-pick, revert, rebase and pull refused for local changes show the
  engine text ("`stash.push`", "--autostash", "niente modificato") in a dialog at the top of
  the window with only "Close". The UI recognizes these refusals (message keys
  `engine-*-local-changes`) and shows its own text ("You have uncommitted changes in
  src/lib.rs: commit or stash them, then try again") with a "Stash…" button that opens the stash
  dialog, and "Close"; the dialog is centered like the others. The engine texts (CLI, MCP) do
  not change.
- **Done when:** merge of `experiment` into `main` with `src/lib.rs` modified shows the new
  dialog in both languages; "Stash…" opens the stash dialog; CI green.
- **Done:** `ErrorKind::LocalChanges` (keys `engine-{merge,pick,pull,rebase}-local-changes`, files in
  `error_detail`) → `common-local-changes` in a centered window with "Stash…" (opens the stash dialog)
  and "Close" for merge, cherry-pick, revert and pull; the rebase editor shows the same text inline
  (no button: the stash dialog cannot open above it). Engine texts unchanged.

### U33 — Narrow commit rows keep the branch names
- [x] **Agent:** `egui-ui`
- **Goal:** in a narrow center the ref badges are dropped first and only "+2" is left while
  the message stays whole. The message is cut first (ellipsis, at least ~8 characters), then the
  badges collapse to the first one + "+n".
- **Done when:** at 760x520 the `experiment` and `feature/docs` badges are visible; CI green.
- **Done:** the ref labels take the row minus the first 8 characters of the subject (or the
  whole subject if shorter); when they do not fit the first one stays and the rest is "+N"
  (`shown_labels` in `view/commits.rs`, unit test).

### U34 — Details panel at narrow width
- [x] **Agent:** `egui-ui`
- **Goal:** at 760x520 the author email wraps in the middle ("nicola.brl94@ / gmail.com") and
  the committer is cut under the separator. Name and email are cut with an ellipsis on one line
  (whole in the tooltip) and the panel scrolls instead of clipping.
- **Done when:** at 760x520 author and committer are readable and not clipped; CI green.
- **Done:** author and committer are one line cut with "…" (whole on hover); the header area
  leaves the file list only the rows it needs (at most half the panel) and scrolls beyond that.

## Phase U4 — Second review: everyday use

### U40 — Keyboard shortcuts
- [x] **Agent:** `egui-ui`
- **Goal:** only Ctrl+F, Ctrl+,, Ctrl+Enter and the arrows exist. Add Ctrl+Z / Ctrl+Shift+Z
  (and Ctrl+Y) for undo/redo, F5 refresh, Ctrl+Shift+L pull,
  Ctrl+Shift+P push, Ctrl+Shift+T fetch, Ctrl+Tab / Ctrl+Shift+Tab between tabs, Space stages /
  unstages the selected file of the stage panel; not while a text field has the keyboard. The
  toolbar tooltips show the shortcut. Help gets "Keyboard shortcuts" with the list.
- **Done when:** unit tests of the key → action mapping; Help → Keyboard shortcuts shows the
  list in both languages; CI green.
- **Done:** one table in `crates/ui/src/keys.rs` (key → command, tooltips, Help → "Keyboard
  shortcuts"); each shortcut becomes the button's own `Action` with the button's gates.

### U41 — Sync counts on Pull and Push
- [ ] **Agent:** `egui-ui`
- **Goal:** "main ↑1 ↓1" is only in the small label at the top left; Pull and Push show
  nothing. Pull shows a "↓n" badge and Push "↑n" (from the current branch's upstream); the Fetch
  tooltip says when the last fetch ran ("last fetch: 3 min ago", from the auto-fetch state).
- **Done when:** on the demo repo after a commit Pull shows ↓1 and Push ↑1; CI green.

### U42 — Commit is the primary button
- [x] **Agent:** `egui-ui`
- **Goal:** the Commit button looks the same enabled or disabled. Enabled it uses the accent
  fill (as the selected row), disabled it stays grey.
- **Done when:** screenshots with an empty and a typed message differ visibly, dark and light;
  CI green.
- **Done:** enabled, Commit has the selected row's fill and text (`theme::primary`); disabled
  it stays egui's grey.

### U43 — "Discard all…" apart from "Stage all"
- [x] **Agent:** `egui-ui`
- **Goal:** "Discard all…" sits next to "Stage all" with the same weight: a misclick opens
  the destructive dialog. "Stage all" stays as a button; "Discard all…" goes into a "⋯" menu of
  the Unstaged section (with the destructive red text, as in the file context menu).
- **Done when:** screenshot of the section; the dialog is still reachable; CI green.
- **Done:** "Discard all…" is in the "⋯" menu at the edge of the Unstaged header, in the
  destructive red (grey when nothing can be discarded); "Stage all" stays a button.

### U44 — Commit context menu: only what makes sense
- [x] **Agent:** `egui-ui`
- **Goal:** on an ancestor of the current branch the menu offers "Merge into main" and
  "Cherry-pick onto main", which do nothing useful; it lacks "Copy commit id" and "Checkout this
  commit" (detached). Merge is hidden for ancestors of HEAD, cherry-pick for commits already in
  HEAD (ancestors); add "Copy commit id" (clipboard) and "Checkout this commit…" (with the
  existing checkout confirmation of a detached HEAD, or hidden if the engine has no such
  command).
- **Done when:** menu on an ancestor of main has no merge/cherry-pick; "Copy commit id" puts
  the full id in the clipboard; CI green.
- **Done:** merge and cherry-pick hidden for HEAD and its ancestors (walk of the loaded commits, only
  while the menu is open); "Copy commit id" copies the full id. "Checkout this commit…" skipped: the
  engine has no detached checkout (`branch.checkout` takes a branch name).

### U45 — Disabled entries say why
- [x] **Agent:** `egui-ui`
- **Goal:** "Delete" on the current branch and "Pull (fast-forward)" on another branch are
  greyed without a reason; the same pull is called "Pull" on `main` and "Pull (fast-forward)"
  elsewhere. Every disabled context-menu entry has a tooltip with the reason (current branch,
  no upstream, ...); the pull entry has one name with the detail in the tooltip.
- **Done when:** hover on the disabled entries shows the reason, both languages; CI green.
- **Done:** the disabled entries already had `on_disabled_hover_text` (checked on Xvfb: Pull without
  upstream, Delete on the current branch); the pull entry is now "Pull" everywhere, fast-forward detail
  in the tooltip.

### U46 — Stable lane colors
- [ ] **Agent:** `egui-ui`
- **Goal:** after a commit the lanes change colors (`origin/main` blue → orange,
  `feature/docs` green → orange) because the color follows the lane index. The color of a
  lane follows the branch at its tip (a hash of the name without the remote prefix, so `main`
  and `origin/main` match), with the lane index only for lanes without a ref; the badge uses the
  same color.
- **Done when:** unit test: the same branch keeps its color when a commit is added on top of
  another lane; `main` and `origin/main` share it; CI green.

### U47 — Search keeps the selection
- [x] **Agent:** `egui-ui`
- **Goal:** Ctrl+F then Esc leaves the found commit selected instead of "Uncommitted
  changes": the stage panel disappears. Closing the search with Esc gives back the selection
  from before the search; Enter (or a click) on a result keeps it.
- **Done when:** unit test of the search state; CI green.
- **Done:** `SearchBox` keeps the selection from before Ctrl+F and gives it back on close
  unless Enter, ↑/↓ or a click/key in the list chose a result (`search_tests.rs`).

## Phase U5 — Second review: polish

### U50 — Wording and duplicates
- [ ] **Agent:** `egui-ui`
- **Goal:** the Italian commit hint is still in the infinitive ("mettere in stage… oppure
  attivare"); "Stage di tutto" is not parallel to "Togli tutto dallo stage" ("Metti tutto in
  stage"). The discard dialog shows "index" and `refs/project/backup/discard-…` in its body: the
  ref goes into a collapsed "Details". The theme is chosen in two places (File → Theme and
  Settings): only Settings keeps it; the File menu does not cut "Ctrl+,". The status bar does not
  repeat the "n changed files" of the graph row. Closing the last tab keeps the tab strip with
  "+"; a recent repo is one link (name) with the path in grey.
- **Done when:** texts changed in both locales; screenshots; CI green.

### U51 — Light theme contrast
- [ ] **Agent:** `egui-ui`
- **Goal:** in the light theme text fields and disabled buttons almost disappear (the subject
  field has no visible border). Text fields get a visible border and background, disabled
  text reaches at least 3:1 against the panel.
- **Done when:** light screenshot of the stage panel shows the fields; CI green.

### U52 — Diff and file menu discoverability
- [x] **Agent:** `egui-ui`
- **Goal:** "Stage selected lines (0)" and "Deselect" stay on screen without a selection and
  nothing says that lines are selected by clicking them. Without a selection the bar shows a
  hint ("Click lines to select them") instead of the two buttons. The file context menu gets
  "Copy path".
- **Done when:** screenshots with and without a selection; "Copy path" fills the clipboard;
  CI green.
- **Done:** without a selection the bar shows "Click lines to select them" (same height as the
  buttons); "Copy path" in the file menu of the stage area and of the commit details.

### U53 — Settings: UI scale and date format
- [x] **Agent:** `egui-ui`
- **Goal:** settings have only theme, language and remotes. Add the UI scale (90–150%, also
  Ctrl+= / Ctrl+- / Ctrl+0) and the date format of the commit list (absolute / relative
  "3 h ago"), persisted like the theme.
- **Done when:** both settings persist across a restart; CI green.
- **Done:** `scale` (90–150%, Ctrl+= / Ctrl+- / Ctrl+0 in place of egui's zoom keys) and
  `dates` (absolute / relative) in the settings, saved with the theme.

### Left for later (they need new engine commands or `Platform` capabilities)

Discard a hunk / selected lines (new write command with backup), "Add to .gitignore", git
identity (`user.name` / `user.email`) in the settings, diff options (ignore whitespace, context
lines), "Open file" / "Show in folder" (a `Platform` capability). Each goes in
[ROADMAP-git.md](ROADMAP-git.md) with its contract before the UI.
