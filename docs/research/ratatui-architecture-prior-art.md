# Ratatui architecture prior art

Research ticket: what application architecture do mature ratatui apps use, and
what should gitchange copy? Sources studied 2026-07-29: ratatui official docs
(ratatui.rs), ratatui official templates, gitui source (gitui-org/gitui,
master), television source (alexpasmantier/television, main), yazi
(sxyazi/yazi, main + yazi-rs.github.io docs). All claims cited inline.

Context: gitchange is a lazygit-style panel-stack TUI (numbered left panels,
dominant diff, contextual keybars, centered popups, a hunk mode) — see
`docs/kickoff/wayfinder-brief.md` §3/§6.

---

## 1. Ratatui's own recommended patterns

The book documents three app patterns, without prescribing one
([ratatui.rs/concepts/application-patterns/](https://ratatui.rs/concepts/application-patterns/)):

- **The Elm Architecture (TEA)** — `Model` struct, `Message` enum,
  `fn update(model: &mut Model, msg: Message) -> Option<Message>` (messages
  can chain), `fn view(model: &mut Model, frame: &mut Frame)`. Pure view
  ("for a given state of the model, it should always produce the same UI
  representation"). Noted constraint: immediate-mode view only learns the
  drawable area at render time, so scroll/size-dependent logic needs stored
  size or resize events.
  ([the-elm-architecture](https://ratatui.rs/concepts/application-patterns/the-elm-architecture/))
- **Component architecture** — trait-based; each component co-locates
  `handle_events`/`handle_key_events`/`handle_mouse_events` → returns
  `Action`, `update(action)`, `render(f, rect)`. Stated benefit:
  "incentivizes co-locating the `handle_events`, `update` and `render`
  functions on a component level."
  ([component-architecture](https://ratatui.rs/concepts/application-patterns/component-architecture/))
- **Flux** — unidirectional central-store variant; least used in the wild.
  ([flux-architecture](https://ratatui.rs/concepts/application-patterns/flux-architecture/))

**Event handling recipe** ([recipes/apps/terminal-and-event-handler](https://ratatui.rs/recipes/apps/terminal-and-event-handler/)):
a dedicated task/thread produces an app-level `Event` enum
(`Tick`, `Key(KeyEvent)`, `Resize(u16,u16)`, `Mouse`, `FocusGained/Lost`,
`Paste`, …) over an unbounded mpsc channel; the tokio flavour uses
crossterm's `EventStream` with `tokio::select!` and a `Tui` wrapper exposing
`async fn next()`.

**Official templates** ([github.com/ratatui/templates](https://github.com/ratatui/templates)):
hello-world / simple / simple-async / event-driven / event-driven-async /
component. The **component template** is the de-facto standard scaffold for
non-trivial apps:

- `App::run` is `async` on tokio; a single unbounded mpsc `Action` channel;
  loop shape: `handle_events(&mut tui).await` → map `Event`→`Action` → drain
  `action_rx.try_recv()` → `component.update(action)` per action → render on
  `Action::Render`. Tick and frame rates configure the `Tui`.
  ([templates/main/component/template/src/app.rs](https://github.com/ratatui/templates/blob/main/component/template/src/app.rs))
- Component trait: `register_action_handler(tx: UnboundedSender<Action>)`,
  `register_config_handler(config)`, `init(area)`,
  `handle_events(Option<Event>) -> Result<Option<Action>>` (+ key/mouse
  specialisations), `update(Action) -> Result<Option<Action>>`,
  `draw(&mut Frame, Rect)`.
  ([templates/main/component/template/src/components.rs](https://github.com/ratatui/templates/blob/main/component/template/src/components.rs))

Takeaway: ratatui's official ecosystem converges on **event → action →
update → draw** with components as trait objects and channels between
stages; the choice of tokio vs threads is left open.

---

## 2. gitui — closest prior art (ratatui + git)

gitui does **not** use tokio. It is a synchronous main loop over crossbeam
channels, with std threads + a rayon pool for background git work. This is
the architecture most directly transferable to gitchange.

### 2.1 Two-crate split

Repo splits UI (`src/`) from git engine (`asyncgit/` crate). `asyncgit`
"provides non-blocking access to Git operations … while keeping the user
interface responsive" and exposes per-domain job types (`AsyncStatus`,
`AsyncDiff`, `AsyncLog`, `AsyncBlame`, `AsyncPush`, …) plus one notification
enum `AsyncGitNotification` (`Status`, `Diff`, `Log`, `Push`, `Pull`,
`Blame`, `Branches`, `Tags`, `FinishUnchanged`, …).
([asyncgit/src/lib.rs](https://github.com/gitui-org/gitui/blob/master/asyncgit/src/lib.rs))

### 2.2 Event loop: crossbeam `Select` over six channels

`src/main.rs` multiplexes all inputs with crossbeam's `Select`:

```rust
fn select_event(
    rx_input: &Receiver<InputEvent>,        // keyboard/mouse thread
    rx_git: &Receiver<AsyncGitNotification>, // asyncgit results
    rx_app: &Receiver<AsyncAppNotification>, // app-level async (syntax highlight etc.)
    rx_ticker: &Receiver<Instant>,           // periodic tick
    rx_notify: &Receiver<()>,                // fs watcher
    rx_spinner: &Receiver<Instant>,          // spinner redraw
) -> Result<QueueEvent>
```

All are folded into a `QueueEvent` enum; loop blocks on `sel.select()`, routes
to handlers, then redraws when `app.requires_redraw()`.
([src/main.rs](https://github.com/gitui-org/gitui/blob/master/src/main.rs))

### 2.3 Input thread

A dedicated std thread polls crossterm and sends `InputEvent` over an
unbounded crossbeam channel (`thread::spawn(move || Self::input_loop(...))`).
Polling can be paused via `set_polling(bool)` on a `NotifiableMutex` — used
when shelling out to an external editor. Poll cadence adapts (fast 100ms
after activity, 10s idle).
([src/input.rs](https://github.com/gitui-org/gitui/blob/master/src/input.rs))

### 2.4 Component trait — the contextual-keybar mechanism

([src/components/mod.rs](https://github.com/gitui-org/gitui/blob/master/src/components/mod.rs))

```rust
pub trait DrawableComponent {
    fn draw(&self, f: &mut Frame, rect: Rect) -> Result<()>;
}
pub trait Component {
    fn commands(&self, out: &mut Vec<CommandInfo>, force_all: bool) -> CommandBlocking;
    fn event(&mut self, ev: &Event) -> Result<EventState>; // Consumed | NotConsumed
    fn focused(&self) -> bool { false }
    fn focus(&mut self, _focus: bool) {}
    fn is_visible(&self) -> bool { true }
    fn hide(&mut self) {}
    fn show(&mut self) -> Result<()> { Ok(()) }
}
```

Two things gitchange needs fall straight out of this:

- **Contextual keybar**: `App::update_commands()` runs `command_pump` across
  all components; each visible/focused component contributes `CommandInfo`
  entries (with enabled/visible flags and `.order()` weights), and
  `CommandBlocking` lets a modal component stop lower components from
  contributing. The bottom keybar is just the render of that aggregated list.
  ([src/app.rs](https://github.com/gitui-org/gitui/blob/master/src/app.rs))
- **Event dispatch = component order**: `event_pump(&ev, self.components_mut())`
  walks components until one returns `Consumed`. The `accessors!` macro lists
  **all popups before the tabs** (`…log_search_popup, fuzzy_find_popup,
  msg_popup, confirm_popup, commit_popup, … revlog, status_tab, files_tab,
  stashing_tab, stashlist_tab`), so a visible popup consumes keys before any
  panel sees them — modality without a separate mode state machine.
  ([src/app.rs](https://github.com/gitui-org/gitui/blob/master/src/app.rs))

### 2.5 Popups

`App` owns ~28 popups as named struct fields (no dynamic popup stack);
macros (`accessors!`, `setup_popups!`) enumerate them. Draw order: top bar →
active tab (skipped if a fullscreen popup is open) → `draw_popups(f)` last,
so popups overlay. Quit is blocked while `any_popup_visible()`. Centering is
each popup's own concern (shared `ui` rect helpers).
([src/app.rs](https://github.com/gitui-org/gitui/blob/master/src/app.rs))

### 2.6 Internal event queue

Components hold a shared `Queue`; they push `InternalEvent` variants
(`ShowErrorMsg`, `OpenCommit`, `Push`, `TabSwitch`, …) instead of calling
into each other. `App::process_queue()` drains it after input handling; each
event returns `NeedsUpdate` bitflags (diff refresh, command refresh, full
redraw) merged into one update pass. This is how a files panel asks the app
to open a popup without holding a reference to it.
([src/app.rs](https://github.com/gitui-org/gitui/blob/master/src/app.rs))

### 2.7 Background jobs: rayon pool + last-request-wins

`asyncgit`'s `AsyncJob` trait runs jobs via `rayon_core::spawn`; results go
back over a crossbeam `Sender<J::Notification>` (`send()` reserved for
progress; final notification returned by `run`). `AsyncSingleJob` "will only
queue up **one** `next` job. It keeps overwriting the next job until it is
actually taken" — i.e. per job-kind, latest request wins and stale
re-computations are dropped. This is exactly the shape needed for re-diffing
on every keystroke-burst of working-tree changes.
([asyncgit/src/asyncjob/mod.rs](https://github.com/gitui-org/gitui/blob/master/asyncgit/src/asyncjob/mod.rs))

### 2.8 Filesystem watching

`notify` + `notify-debouncer-mini`, 2-second debounce, recursive watch of the
repo. Two threads: watcher (std `mpsc` internally) and a forwarder that sends
a bare `()` over a crossbeam channel into the main `Select`. The signal
carries no payload — it just triggers a status/diff refresh cycle.
([src/watcher.rs](https://github.com/gitui-org/gitui/blob/master/src/watcher.rs))

### 2.9 Keybindings

`KeyConfig { keys: KeysList, symbols: KeySymbols }`. Both load from RON files
(`key_bindings.ron`, `key_symbols.ron`) with **partial override** — user
specifies only the bindings/symbols they change. Components compare events
against named fields (`key_match(ev, key_config.keys.some_action)`).
`get_hint(ev)` renders a key as keybar text
(`format!("{}{}", self.get_modifier_hint(ev.modifiers), self.get_key_symbol(ev.code))`)
— so the keybar labels come from the same config as the bindings, and the
display glyphs for keys are themselves user-replaceable tokens.
([src/keys/key_config.rs](https://github.com/gitui-org/gitui/blob/master/src/keys/key_config.rs),
[src/keys/mod.rs](https://github.com/gitui-org/gitui/blob/master/src/keys/mod.rs))

### 2.10 Theming

`Theme` is a flat struct of ~21 semantic `Color` tokens (`selected_tab`,
`command_fg`, `selection_bg`, `selection_fg`, `cmdbar_bg`, `disabled_fg`,
`diff_line_add`, `diff_line_delete`, `diff_file_added/removed/moved/modified`,
`commit_hash`, `commit_time`, `commit_author`, `danger_fg`, `push_gauge_bg/fg`,
`tag_fg`, `branch_fg`, `block_title_focused`) plus `line_break: String` and
`syntax: String`. It derives `Serialize, Deserialize` **and a `Patch` macro**
(`ron` + partial-override): `init()` loads a `ThemePatch` and applies it over
defaults, with a legacy full-theme fallback. Components never touch raw
colours — they call methods returning `ratatui::Style`
(`theme.diff_line(kind)`, `theme.commit_hash()`, `theme.item(selected)`).
([src/ui/style.rs](https://github.com/gitui-org/gitui/blob/master/src/ui/style.rs))

---

## 3. Television — tokio action-loop with modes

Television (fuzzy finder TUI, real mode/popup complexity) is the strongest
example of the official component-template lineage taken to production.
([github.com/alexpasmantier/television](https://github.com/alexpasmantier/television))

- **Runtime**: tokio. Unbounded mpsc channels for actions (`action_tx/rx`),
  events (`event_rx`), render tasks (`render_tx`), and UI-state feedback from
  the renderer (`ui_state_rx`). Rendering runs as its **own tokio task**
  consuming `RenderingTask` messages — draw is decoupled from the update
  loop. ([television/app.rs](https://github.com/alexpasmantier/television/blob/main/television/app.rs))
- **Main loop**: `tokio::select!` over `event_rx.recv_many(&mut event_buf, N)`
  and `action_rx.recv_many(&mut action_buf, N)` — events/actions are
  **batched**, then processed sequentially; the action drain "shouldn't block
  if no actions are available".
  ([television/app.rs](https://github.com/alexpasmantier/television/blob/main/television/app.rs))
- **Modes drive keybinding lookup**: `Mode { Channel, RemoteControl,
  ActionPicker }`; `convert_event_to_actions` resolves
  `input_map.get_actions_for_key(&keycode, &self.television.mode)` first,
  unmapped printable keys fall through to `Action::AddInputChar(c)`. Mode is
  a first-class field, not implied by component visibility.
  ([television/app.rs](https://github.com/alexpasmantier/television/blob/main/television/app.rs),
  [television/television.rs](https://github.com/alexpasmantier/television/blob/main/television/television.rs))
- **Async results wake the loop**: the nucleo matcher gets a `Notify` closure
  capturing `action_tx` that sends `Action::MatcherUpdated`, so results
  render immediately rather than on the next tick; previews arrive over a
  channel polled in `update_preview_state()`. Render throttling is adaptive
  (always render first 10 ticks; every 25 ticks idle; every 3 while a channel
  is running; immediately on `MatcherUpdated`).
  ([television/television.rs](https://github.com/alexpasmantier/television/blob/main/television/television.rs))
- **Theming**: standalone TOML token files in `themes/`, flat `_fg`/`_bg`
  semantic tokens (`background`, `border_fg`, `text_fg`, `dimmed_text_fg`,
  `result_name_fg`, `selection_fg/bg`, `match_fg`, `preview_title_fg`,
  `channel_mode_fg/bg`, …).
  ([themes/default.toml](https://github.com/alexpasmantier/television/blob/main/themes/default.toml))

---

## 4. Yazi — the maximal end of the spectrum

Yazi (file manager) shows what a large ratatui app becomes: a ~20-crate
workspace (`yazi-core`, `yazi-fm`, `yazi-scheduler` task scheduling,
`yazi-actor` actor-based concurrency, `yazi-plugin` Lua, `yazi-dds`
client-server event bus, `yazi-watcher`, `yazi-config`, …) on tokio with
non-blocking async I/O throughout.
([github.com/sxyazi/yazi](https://github.com/sxyazi/yazi))

Two subsystems are directly relevant even though the overall scale is
overkill for gitchange:

- **Layered keymap** ([yazi-rs.github.io/docs/configuration/keymap](https://yazi-rs.github.io/docs/configuration/keymap)):
  bindings grouped by UI-context layer (`[mgr]`, `[tasks]`, `[spot]`,
  `[pick]`, `[input]`, `[confirm]`, `[cmp]`, `[help]`); each binding is
  `{ on = "<C-a>", run = "command", desc = "…" }` where `desc` feeds the
  help/which-key menu; user config uses `prepend_keymap` (overrides,
  first-match-wins) / `append_keymap` (fallbacks) rather than replacing
  defaults. Layer ≈ gitchange's normal-vs-hunk-mode-vs-popup contexts.
- **Theme + flavors** ([yazi-rs.github.io/docs/configuration/theme](https://yazi-rs.github.io/docs/configuration/theme)):
  `theme.toml` sections per UI region (`[mgr]`, `[mode]`, `[status]`,
  `[tabs]`, `[filetype]`, `[icon]`, dialog sections); style objects
  `{ fg = "#e4e4e4", bg = "black", bold = true }`; a `[flavor]` section
  selects packaged themes per light/dark; `prepend_*`/`append_*` lets users
  extend icon/style rules without replacing defaults. Icons/glyphs are theme
  data, not code — the precedent for the brief's replaceable-glyph
  requirement.

---

## 5. Cross-cutting answers

**Runtime choice.** Both camps are proven: gitui = std threads + crossbeam +
rayon, no async runtime; television/yazi/official templates = tokio.
gitchange's background work is exactly gitui-shaped: blocking libgit2/gix
calls, `notify` callbacks, crossterm polling — none of it is natively async,
so tokio would mostly wrap blocking work in `spawn_blocking`. gitui
demonstrates the threads+crossbeam design carries a full git TUI (including
push/pull progress) without jank.

**Feeding the UI without jank.** Common pattern across all three: background
work never touches UI state directly; it sends a **notification enum** over a
channel into the one place that owns state, which then re-reads/re-renders.
Staleness is handled by last-request-wins job slots (gitui
`AsyncSingleJob`) or wake-on-result actions plus render throttling
(television). FS watching is `notify` + debouncer on its own thread emitting
a payload-free "something changed" signal (gitui uses 2s debounce).

**Keybinding dispatch with contextual keybars.** Two mechanisms, composable:
(a) gitui — visibility-ordered `Consumed`-chain plus per-component
`commands()` aggregation that *is* the keybar, with `CommandBlocking` for
modality; (b) television/yazi — explicit mode/layer keyed keymap tables with
per-binding `desc`. gitui's is the best fit for a lazygit clone because the
keybar and the dispatch derive from the same component list, so they can't
drift.

**Popups.** gitui: popups are components owned by App, enumerated before
panels for input priority, drawn last for z-order, each computing its own
centered rect. No popup manager/stack abstraction needed at this scale.

**Theming/glyph tokens.** Convergent pattern: flat struct/table of
**semantic** tokens (role names, not colour names), defaults in code,
user file applies a **partial patch** (gitui `ron` Patch derive; yazi/tv
TOML with prepend/append), components consume only `Style`-returning
methods. Glyphs live in the same config layer (gitui `key_symbols.ron`,
yazi `[icon]`).

---

## 6. Recommended architecture sketch

> **Status: recommendation only — to be ratified by a human in a later
> session.** Modelled primarily on gitui, with television's explicit mode
> field and yazi's keymap/theme file conventions.

### Crate/module split

- `gitchange` (bin): TUI — app loop, components, keys, theme.
- `gitchange-core` (lib, or a strictly-bounded module to start): the
  changelist engine — repo access, status/diff, hunk identity + membership,
  index synthesis, persistence. No ratatui imports. Mirrors gitui/asyncgit;
  keeps the hardest problems (brief §6) testable without a terminal.

### Runtime & event loop (gitui-style, no tokio)

Single sync main loop; crossbeam `Select` over:

1. `rx_input: Receiver<InputEvent>` — dedicated std thread polling
   crossterm (pausable for external `$EDITOR` commit messages).
2. `rx_engine: Receiver<EngineNotification>` — results from background jobs
   (`Status`, `Diff`, `HunkMembership`, `CommitDone`, `Error`, …).
3. `rx_watcher: Receiver<()>` — `notify` + `notify-debouncer-mini` thread;
   debounce ~500ms–2s (tune; gitui uses 2s), signal triggers a status+diff
   job, which re-runs hunk re-association.
4. `rx_ticker` — coarse periodic tick (spinner, lazy refresh).

Rationale over tokio: all background work is blocking git/FS calls; fewer
moving parts; avoids async-trait friction in the component layer. If the
team prefers tokio (template familiarity, future network features), the
design ports 1:1 — swap `Select` for `tokio::select!` and run jobs via
`spawn_blocking`; this is a reversible decision.

### State ownership & components

- `App` owns everything: `Vec`-less named fields for the four panels
  (`status_panel`, `changelists_panel`, `files_panel`, `commits_panel`),
  `diff_panel`, and each popup (`move_popup`, `confirm_popup`, `input_popup`,
  `help_popup`, `commit_popup`, `msg_popup`); an `accessors!`-style macro
  enumerates them **popups first**.
- gitui-shape `Component` trait: `draw`, `event(&Event) -> EventState`,
  `commands(&mut Vec<CommandInfo>, force_all) -> CommandBlocking`,
  `focused/focus/is_visible/hide/show`. The keybar renders the aggregated
  `commands()` output — contextual keybars for free, including hunk mode.
- One explicit mode field on App (television-style):
  `enum Focus { Panel(u8), Diff }` + `enum AppMode { Normal, HunkMode }`
  (popups modal via visibility/ordering, not a mode variant). Hunk mode
  flips the diff panel's `commands()`/`event()` behaviour and the keybar
  follows automatically.
- Cross-component requests via a shared `Queue<InternalEvent>`
  (`OpenMovePopup { target }`, `ChangelistCreated`, `ShowError`, …) drained
  each loop, returning `NeedsUpdate` bitflags (STATUS | DIFF | COMMANDS |
  REDRAW). Components never hold references to each other.

### Background-task channel design

- `Engine` owns per-kind job slots: `AsyncSingleJob<StatusJob>`,
  `AsyncSingleJob<DiffJob>`, `AsyncSingleJob<MembershipJob>` — rayon (or a
  2–3 thread pool) execution, crossbeam `Sender<EngineNotification>` back to
  the loop, **last-request-wins** so a burst of watcher events collapses to
  one recompute. Mutating ops (commit, stage, move-hunk) run as one-shot
  jobs with progress notifications.
- UI never blocks on git: it renders last-known state + spinner until the
  notification lands.

### Keys, theme, glyphs

- `KeyConfig` à la gitui: named-action struct (`focus_panel_n`, `move_item`,
  `stage_item`, `enter_hunk_mode`, …), TOML/RON file applying a partial
  patch over defaults, `key_match()` in components, `get_hint()` feeding
  keybar labels. Consider yazi-style `desc` strings for the `?` help popup.
- `Theme`: flat semantic token struct (`selection_bg`, `diff_line_add`,
  `diff_line_delete`, `hunk_tag_fg`, `hunk_dimmed_fg`, `unassigned_warn_fg`,
  `active_marker_fg`, `cmdbar_bg`, `disabled_fg`, `popup_border`, …) with a
  partial-override user file and `Style`-returning methods only. Satisfies
  the brief's "non-opinionated, tokenised" requirement.
- `Glyphs`: sibling struct of string tokens (`staged: "●"`,
  `partial: "◐"`, `unstaged: "○"`, `active: "*"`, `warn: "!"`,
  `hunk: "≡"`, `line_break`), same partial-override mechanism — direct
  analogue of gitui's `line_break`/`KeySymbols` and yazi's icon tables.

### What we deliberately do NOT copy

- Yazi's actor/DDS/plugin machinery and 20-crate split — wrong scale.
- Television's separate rendering task — gitui's draw-in-loop is simpler and
  sufficient at git-TUI refresh rates.
- gitui's 28-popup sprawl — start with the ~6 popups the brief needs; keep
  the macro so adding one stays one-line-per-site.
