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
- [ ] **Agent:** `egui-ui`
- **Goal:** when the window gets focus back (also while it is resized, on Windows and GNOME) the
  automatic refresh re-reads the status, `follow_diff` calls `DiffPane::reload` and that drops
  the loaded diff (`answer = None`) and the line selection: the view flashes "Loading the
  diff…", loses the scroll, the `+n -m` counters and the "Stage selected lines" bar. `reload`
  keeps the old diff on screen until the new one arrives; if the new one is the same, the
  selection stays, otherwise it is cleared.
- **Done when:** unit test on `DiffPane` (reload keeps the answer, same diff keeps the
  selection, different diff clears it); on Xvfb focus lost and regained with an unstaged diff
  open shows no loading state; CI green.

## Phase U1 — Layout and resizing

### U10 — Side panels follow the window
- [ ] **Agent:** `egui-ui`
- **Goal:** the side panels' `max_size` (a third of the window) clamps their width when the
  window shrinks and egui stores the clamped width: when the window grows again they stay
  narrow. At small widths the center (graph, diff) gets less room than the two panels together
  (~280 px of 760). The panels keep the width chosen by the user and are only clamped while
  drawing, and the center keeps at least ~40% of the window: the panels shrink first, down to
  their minimum.
- **Done when:** 1200 → 900 → 1200 gives back the starting widths; at 760x520 the center is
  wider than each side panel; CI green.

### U11 — Headers without overlaps
- [ ] **Agent:** `egui-ui`
- **Goal:** in a narrow center the diff header draws "in commit c13e0f9" / "unstaged" under the
  `+n -m` counters, the "Stage hunk" button covers the `@@ … @@` text and the split-view hint is
  cut at the right edge. The counters and the view toggles keep their place on the right and the
  description is cut with an ellipsis before them; the hunk header text is clipped before its
  button; the hint wraps.
- **Done when:** at 760x520 no text overlaps another in the diff of a commit, of an unstaged
  file and in the split view; CI green.

### U12 — Stage panel: titles before buttons, no jump with a dialog open
- [ ] **Agent:** `egui-ui`
- **Goal:** "Unstaged (2)" becomes "Unstage…" ("Non in sta…" in Italian already at the default
  width) because the buttons on the same row take the room first, and "Uncommitted changes" is
  cut beside "Stash…". The section title keeps its full text; when the row is too narrow the
  buttons go on the next line. With a dialog open the commit box moves ~13 px down and its hint
  disappears: the layout is the same whether a dialog is open or not.
- **Done when:** at 760x520, in both languages, the titles of the stage panel are whole; a
  screenshot with the stash dialog open and one without show the commit box at the same place;
  CI green.

## Phase U2 — Text

### U20 — A type scale
- [ ] **Agent:** `egui-ui`
- **Goal:** the font sizes are written by hand in each view (11, 11.5, 12, 12.5, 13, 13.5) and
  `TextStyle::Small` is egui's default 9 pt, used for the status bar, the branch label at the top
  left, the stash sub-line, the commit hint and counter: too small next to the body text. The
  theme defines the sizes once (body, small at least 11 px, monospace, heading) and the views
  use them.
- **Done when:** no `FontId::proportional(<number>)` / `monospace(<number>)` left outside the
  theme; the small texts are readable on the Xvfb screenshot; CI green.

### U21 — Wording and repeated header
- [ ] **Agent:** `egui-ui`
- **Goal:** Italian "Unstage di tutto" becomes "Togli tutto dallo stage" and the hint "Scrivere
  il messaggio del commit" an imperative ("Scrivi …"); "Ctrl+Comma" is shown as "Ctrl+," in both
  languages, as "Ctrl+Enter" is shown with its key. The diff of one file shows its name twice
  (bar "README.md in commit …" and the file row "README.md modified +2 -0"): with a single file
  the bar does not repeat what the file row says.
- **Done when:** the texts above changed in both locales; i18n tests green; screenshot of a
  one-file diff; CI green.
