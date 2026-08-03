# ADR coverage audit (v0.1 exit)

Every ADR-mandated behaviour mapped to a covering test or a recorded
gap — the final pass required by issue #36, taken at commit `5a19b68`
on 2026-07-31. Compiled by four parallel audits (three ADRs each);
"GAP"/"Partial" verdicts were checked against test bodies, not just
names. The consolidated gap list and the accepted deviations live in
[v0.1-exit-record.md](v0.1-exit-record.md#4-adr-coverage-pass).

## ADR 0001 — Hunk identity: content-anchored records, recomputed at refresh

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Record shape: path, old/new coords, changelist, verbatim anchor (+/- lines plus context, plain text) | `tests/matcher.rs::stash_then_pop_round_trips_membership_through_dormancy` (reads `state.json` record fields); `tests/hunk_universe.rs::hunk_lines_carry_verbatim_content_with_origins` | **Partial.** Field presence and verbatim line origin/content are asserted; no test asserts the stored anchor *contains ~3 context lines* — anchor fidelity is only exercised indirectly (exact-match round-trips would fail if anchors were wrong). |
| Membership re-derived at every refresh; matcher pure fn of (records, fresh diff); records mutate only via completed refresh or user action | `tests/matcher.rs::refresh_does_not_rewrite_the_state_file_when_records_are_unchanged`; whole matcher suite is refresh-driven | Purity is an architectural property; the no-rewrite test covers the "mutate only on change" edge. Acceptable indirect coverage. |
| Tier 1: exact content-anchor match survives position moves | `tests/matcher.rs::a_moved_hunk_keeps_membership_via_exact_anchor_match` | Direct. Binary flavour: `an_unchanged_binary_matches_exactly_after_a_move`. |
| Tier 2: overlap inheritance, records shifted by preceding matched hunks' deltas | `tests/matcher.rs::editing_your_own_hunk_keeps_membership_via_overlap`; shift/commutation: `tests/commit.rs::committing_one_changelist_commutes_same_file_records`, `tests/head_moves.rs::a_head_move_touching_only_other_paths_leaves_tier_two_intact` | **Partial.** Overlap inheritance and commit/head-move commutation are direct; the intra-refresh case "fresh hunk above shifts a stored record whose owned hunk was *also edited* (anchor broken)" is not asserted as such. |
| Splits inherit the parent's changelist | `tests/matcher.rs::a_split_hunk_inherits_the_parents_changelist` | Direct, asserts both fragments. |
| Editing your own hunk never sheds membership | `tests/matcher.rs::editing_your_own_hunk_keeps_membership_via_overlap`; `tests/assign.rs::an_assigned_hunk_keeps_its_new_owner_when_edited` | Direct, including with a different changelist active. |
| New hunk, no overlap → active changelist (uniform: live and at launch after offline edits) | `tests/matcher.rs::new_hunks_capture_to_the_active_changelist`; launch-after-offline flavour: `crates/gitchange/tests/cli.rs::status_groups_files_by_changelist_with_unassigned_last` | Direct for both flavours. Capture is noticed once: `a_routine_auto_capture_notices_once`. |
| New hunk overlapping ≥2 changelists → active + notification | `tests/matcher.rs::a_hunk_overlapping_two_changelists_captures_to_active_with_a_notice`; `cli.rs::status_prints_ambiguous_overlap_notices_on_stderr` | Direct, asserts `Notice::AmbiguousOverlap` payload and quiet follow-up refresh. |
| No changelists → unassigned; unassigned holds pre-changelist dirty tree, orphans, hunks assigned to it by hand | `tests/matcher.rs::with_no_changelists_hunks_are_unassigned_and_no_state_file_is_written`, `deleting_a_changelist_orphans_its_hunks_to_unassigned`; `tests/assign.rs::an_explicit_assign_to_unassigned_is_sticky_across_edits` | Direct, all three unassigned populations covered; orphan stickiness asserted. |
| Failure mode visible (active+notify or unassigned), never silent wrong-list | `tests/head_moves.rs::a_shifted_neighbour_goes_dormant_loudly_instead_of_misfiling`; `tests/matcher.rs::dormant_records_never_revive_via_overlap`, `with_no_active_changelist_nothing_notices` | Direct for the head-move and dormancy paths. |
| Determinism: same records + same diff → same membership | (suite-wide property, stated in `tests/matcher.rs` header) | Implicit; every matcher test is table-shaped through `Repo::refresh()`. The quiet-second-refresh checks approximate it. |

## ADR 0002 — Persistence: pure JSON sidecar

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Single pretty-printed JSON at `$GIT_DIR/gitchange/state.json` | `tests/changelists.rs::state_file_is_pretty_json_with_schema_version_at_the_git_path` | Direct: path, pretty-print, `version`, `active`, changelist names. |
| Schema version field; behaviour on unknown version | `state_file_is_pretty_json_with_schema_version_at_the_git_path`; `an_unsupported_schema_version_is_a_loud_error` | Direct (loud error goes beyond the ADR's letter, consistent with its spirit). |
| Atomic write-then-rename | **GAP** | Implemented in `src/state_file.rs` (`TMP_FILE` + `fs::rename`) but no test asserts atomicity, tmp cleanup, or crash-safety. A "no `.tmp` left behind" assertion would be cheap. |
| Held lockfile → fail fast with clear error, never wait | `tests/changelists.rs::a_held_lockfile_fails_fast_with_lock_contention` | Direct; also asserts the failed attempt doesn't steal/remove the lock. |
| No git objects written | **GAP** | Never asserted (e.g. odb object count unchanged across refresh/ops). Low risk given the implementation writes only files, but the ADR states it as a property. |
| Anchors plain text, not compressed | `matcher.rs`/`cli.rs` tests hand-write and read anchors as JSON string arrays | Indirect but adequate: every state-seeding test would break if anchors were compressed. |
| Branch switch: changelists global to working tree, ride across `git switch`; records store no branch | **GAP** | No test performs a `git switch`/`checkout` with dirty owned hunks and asserts membership survives. "No branch field" is only implicit in the schema tests. |
| Worktrees: independent per-worktree state via `--git-path` | `tests/changelists.rs::linked_worktrees_have_independent_state_files` | Direct, including the `.git/worktrees/<id>/gitchange/` location assertion. |
| Strictly local; persistence behind a trait | n/a | Architectural, not behaviourally testable; not counted as a gap. |
| Missing state file / fresh clone → empty state is correct | `tests/changelists.rs::a_missing_state_file_reads_as_no_changelists` | Direct for the missing-file half; clone/gc "non-events" follow trivially. |
| Unmatched record → dormant, retained; revival only tier-1 exact, never overlap | `tests/matcher.rs::stash_then_pop_round_trips_membership_through_dormancy`, `dormant_records_never_revive_via_overlap`; binary: `binary_dormant_revival_is_exact_only` | Direct, asserts `dormant_since` in the file both ways. |
| `git stash` → `git stash pop` round-trips membership | `tests/matcher.rs::stash_then_pop_round_trips_membership_through_dormancy`, `dormant_revival_notices_with_a_per_changelist_count` | Direct. |
| gitchange-initiated commits remove their own records | `tests/commit.rs::commit_writes_only_the_changelists_staged_hunks` | Direct: asserts the consumed record is removed, "not left dormant". |
| Dormant records pruned on changelist delete or after 14 days | 14-day: `tests/matcher.rs::dormant_records_prune_after_fourteen_days`; delete: `records_naming_an_unknown_changelist_get_delete_semantics` | 14-day is direct. The delete path is covered via the ghost-changelist test; no test drives `delete_changelist` while a dormant record exists — near-equivalent, mild partial. |

## ADR 0003 — Staging: write-through to the live index, staged state derived

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Stage = genuine apply to live index; unstage = reverse-apply | `tests/staging.rs::staging_one_hunk_of_a_multi_hunk_file_leaves_the_other_unstaged`, `unstaging_one_hunk_leaves_the_sibling_staged` plus the whole `apply_corpus.rs` suite (expected index bytes) | Direct and deep — corpus checks index bytes per op. |
| CLI shell-out fallback for apply edge cases | **Recorded deviation** — trigger covered by `apply_corpus.rs` (green = untriggered) plus `staging.rs::a_refused_apply_reports_apply_failed_and_stages_nothing` | Not implemented, and re-scoped by the ADR from deferred work to a mitigation conditional on an observed `Error::ApplyFailed`. Trigger variant exists and is mapped from the libgit2 `apply` call alone; the corpus is its certification suite. The variant's own contract — path named, libgit2 message verbatim, index untouched — is now asserted via an unwritable odb, which secures the reporting path the trigger must travel without being a trigger event itself (an odb nothing can write to fails `git apply --cached` equally). The payload shapes the ADR feared were probed and stay unreachable. Note the trigger is instrumented on the *index* apply only: the commit path's temp-index apply reports `Backend` (issues #55, #58). |
| No staged bit persisted; staged state derived each refresh via tier-1 match of diff(HEAD↔index) | Derivation: all of `tests/hunk_universe.rs`; schema: `changelists.rs::state_file_is_pretty_json…` | Derivation is direct. "No staged bit in state.json" has no negative assertion, but every hand-written state fixture omits it. |
| Hunk universe = union of worktree and index diffs; every committable hunk visible | `tests/hunk_universe.rs::an_index_only_hunk_after_worktree_revert_is_staged_stale` (the ADR's named hard case), `a_staged_new_file_removed_from_the_worktree_reads_deleted_and_stale` | Direct. |
| Three-valued state: ○ unstaged / ● staged / ◑ staged-stale (both flavours) | `hunk_universe.rs::a_worktree_only_edit_is_a_single_unstaged_hunk`, `an_externally_staged_untouched_hunk_is_staged`, `a_staged_then_edited_hunk_is_staged_stale`, `an_index_only_hunk_after_worktree_revert_is_staged_stale`; binary: `binary_staging_is_derived_by_oid_compare` | Direct, all four table rows. |
| `space` on ◑ sets index := worktree (restage edited; discard index-only) | `tests/staging.rs::space_on_a_staged_stale_hunk_sets_index_to_worktree`, `space_on_an_index_only_hunk_discards_it_from_the_index`; binary: `space_on_a_stale_binary_restages_the_worktree_blob`; TUI: `app.rs::space_in_hunk_mode_toggles_the_selected_hunk` | Direct. |
| Per-file markers ●◐○; staged-stale counts toward ◐ | `hunk_universe.rs::a_file_with_one_staged_and_one_unstaged_hunk_is_partially_staged`; ◑→◐: `a_staged_then_edited_hunk_is_staged_stale`; glyph rendering: `gitchange-tui/tests/render.rs::the_panel_stack_renders_with_the_all_view`, `cli.rs::status_marks_externally_staged_files` | Direct. |
| External staging absorbed, never an error (pre-existing at launch, external `git add`, external `git reset`) | `hunk_universe.rs::an_externally_staged_untouched_hunk_is_staged`; `cli.rs::status_marks_externally_staged_files` (launch case) | **Partial.** `git add` and launch-time absorption direct; no test performs an external `git reset` and asserts absorption. |
| Owned staged hunks derive their record's state; unowned staged hunks follow ADR 0001 assignment | Owned: `tests/commit.rs::commit_writes_only_the_changelists_staged_hunks`; unowned/no-changelists: `cli.rs::status_marks_externally_staged_files` | **Partial.** The "unowned externally *staged* hunk captures to the active changelist (+notice)" combination is not tested — capture tests use worktree-only hunks. Same code path; risk is low. |
| Unstage one hunk while file keeps other staged hunks (index-blob reverse-apply) | `tests/staging.rs::unstaging_one_hunk_leaves_the_sibling_staged`, `unstaging_a_staged_stale_hunk_restores_head_content` | Direct. |

## ADR 0004 — Commit mechanics: temp index via GIT_INDEX_FILE

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Commit builds a temp index (HEAD tree + only this changelist's staged hunks); live index never modified | `crates/gitchange-core/tests/commit.rs::commit_writes_only_the_changelists_staged_hunks` | Asserts HEAD holds only the payload AND the live index still holds both changelists' hunks. Direct. |
| Hooks run natively and see the commit's true content via `git diff --cached` | `commit.rs::a_hook_sees_the_commits_true_content` | Real pre-commit hook captures `git diff --cached`. `#[cfg(unix)]` only — no Windows hook coverage. |
| Every failure changes nothing (index, worktree, state untouched; no commit created); hook stderr surfaced | `commit.rs::hook_rejection_changes_nothing` | Asserts commit count, index content, worktree content, state.json byte-identical; `HookRejected { stderr }` carries hook output. |
| Post-commit consistency: other changelists' derived staged state stays correct, no restoration step | `commit.rs::commit_writes_only_the_changelists_staged_hunks` + `committing_one_changelist_commutes_same_file_records` | Direct. |
| Staged-stale `◑` in payload → warn-and-confirm, never silent | core: `commit.rs::payload_counts_cover_both_stale_flavours`; TUI: `app.rs::a_stale_payload_routes_through_the_warn_overlay` | Both `◑` flavours counted; TUI routes through warn overlay. |
| Align option: index := worktree for the changelist's stale hunks | `commit.rs::align_sets_index_to_worktree_for_the_changelists_stale_hunks` | Covers both flavours. TUI Align→re-check→commit wiring lives in the untested `gitchange-tui/src/lib.rs` run loop. |
| Zero staged hunks → error, frontend offers stage-all-and-commit; `c` never silent-auto-stages | core: `commit.rs::zero_staged_hunks_is_an_error_not_a_commit`; TUI: `app.rs::an_empty_payload_offers_stage_all_with_the_changelists_counts` | Direct on both layers. |
| Unassigned is committable; `c` on `all` view is a no-op with a message | core: `commit.rs::unassigned_commits_like_any_changelist`; TUI: `app.rs::c_on_all_is_a_polite_noop_and_opens_the_flow_scoped` | Core test also pins "no state file grows from committing a pre-changelist tree". |
| Freshness guard: refresh-before-commit; drift returns to confirm step with fresh payload | core: `commit.rs::payload_drift_returns_to_the_confirm_step`; TUI: `app.rs::drift_reconfirms_with_the_message_kept` | Direct. |
| `--no-verify` supported | `commit.rs::no_verify_commits_past_a_rejecting_hook`, paired with `hook_rejection_changes_nothing` | Direct on unix. The two tests share one fixture builder and the same rejecting hook, so the flag is structurally the only difference; the bypass is asserted to produce an ordinary commit (HEAD moves, payload lands, consumed record removed), not a special case. `#[cfg(unix)]` like every hook test — Windows hook coverage is its own gap (issue #63). |
| Fully-consumed records removed explicitly (not dormant) | `commit.rs::commit_writes_only_the_changelists_staged_hunks` | Direct. |
| `◑` records retained; residual diff re-attaches to same changelist at next refresh | `commit.rs::a_residual_stale_hunk_reattaches_after_an_own_commit`, `a_residual_stale_hunk_reattaches_when_the_payload_shifts_it`; contrast: `head_moves.rs::a_residual_staged_stale_hunk_goes_dormant_across_an_external_commit` | Thorough, including the shift case. |
| Baseline stamped in same update as commit (ADR 0012 amendment) | `commit.rs::commit_stamps_the_baseline_in_the_same_update` | Direct. |
| Worktree never touched; full refresh follows the commit | failure side: `hook_rejection_changes_nothing`; success side implicit; post-commit `request_refresh()` sits in the untested TUI run loop | Partial. |
| Emptied changelist kept until explicitly deleted | `commit.rs::commit_writes_only_the_changelists_staged_hunks` | Direct. |
| Amend: same temp-index path, `--amend`, identical guards/bookkeeping | `commit.rs::amend_reuses_the_temp_index_path` | Tip replaced; third changelist's staged hunk excluded and still staged. Message-prefill and hooks-under-amend not directly asserted. |
| Apply failures abort cleanly before any commit exists | `commit.rs::a_refused_temp_index_apply_aborts_before_any_commit_exists`; cleanup of files that exist: `hook_rejection_changes_nothing` | Partial, and permanently so on one axis. The abort is driven by an unwritable object store — the only refusal reachable without a test-only seam, since the applied diff is computed from HEAD's tree and applied straight back to it (issue #58). A *payload* that fails to apply is unreachable through the public ops. Note the error is `Backend`, not `ApplyFailed`: only the index apply maps to that variant. |

## ADR 0005 — Refresh: watcher-driven atomic snapshot recompute

The four performance mitigations (lazy per-file diff detail, capped diff
context, skip huge files, incremental matching) are deliberately
unimplemented — measurement-gated behind the benchmark harness — and are
not gaps.

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| One atomic RefreshJob → single `RefreshComplete` carrying an immutable snapshot; UI swaps whole snapshots | `crates/gitchange-core/tests/engine.rs::touching_a_file_produces_a_refresh_complete` (real fs/notify smoke) + `src/engine.rs::refresh_started_precedes_every_complete` | TUI consumes only via `apply_snapshot`; app.rs snapshot-swap tests exercise it. |
| ~500ms debounce; watcher burst collapses to one recompute | `src/engine.rs::debounce_coalesces_a_burst_into_one_refresh` | Synthetic 25ms config; the ~500ms value untested by design (ADR 0008). |
| Recursive watch includes `.git`; external git ops absorbed via index/HEAD events | Partial: `src/engine.rs::self_loop_filter_matches_the_state_dir_only` asserts `.git/index`/`.git/HEAD` are NOT filtered; linked-worktree extra watch untested | No integration test drives a real external git op through the real watcher. Absorption semantics covered watcher-independently in `tests/head_moves.rs`. |
| Self-loop filter: drop events under `$GIT_DIR/gitchange/` and the temp index | `src/engine.rs::self_loop_events_are_dropped` + `self_loop_filter_matches_the_state_dir_only` | Temp index lives inside the state dir, so the prefix filter covers it; mixed batches still trigger — tested. |
| Belt-and-braces: state file not rewritten when records unchanged | `tests/matcher.rs::refresh_does_not_rewrite_the_state_file_when_records_are_unchanged` | mtime-based, direct. |
| Own mutations fire the RefreshJob directly (never wait on the watcher) | **GAP** (wiring) | `Engine::request_refresh` itself is exercised; the mutation→refresh calls live in the untested `gitchange-tui/src/lib.rs` run loop. |
| `FocusGained` triggers a refresh | **GAP** | Wired in `gitchange-tui/src/lib.rs`, no test. |
| Manual refresh key exists | `crates/gitchange-tui/src/app.rs::quit_and_refresh_actions` (`R` → `Action::Refresh`) | Action→`request_refresh` mapping in lib.rs untested; minor. |
| No periodic polling in healthy path | Indirect: quiet-window asserts in `self_loop_events_are_dropped` and `last_request_wins_…` | Adequate but indirect. |
| Degraded mode: watcher death → notify once, poll ~5s; focus/key/mutation refresh unaffected | `src/engine.rs::watcher_death_degrades_and_polling_keeps_refreshing`; TUI display: `app.rs::conditions_pin_and_self_clear` | Failure-to-initialise flavour (spawn-time degraded) untested. |
| Last-request-wins slot: requests behind a running job collapse to one | `src/engine.rs::last_request_wins_collapses_requests_behind_a_running_job` | Direct (made deterministic during this gate: issue #48). |
| Lock contention retried, never surfaced | `src/engine.rs::lock_contention_is_retried_not_surfaced` | Direct. |
| Mid-refresh: panels keep last snapshot, fully interactive, nothing clears | Partial: `app.rs::a_failed_refresh_keeps_the_last_snapshot_and_modals`; `indicator_defers_until_the_threshold` | No test asserts input handling continues during an in-flight refresh; largely structural. |
| Deferred indicator: none under ~500ms, shown past threshold until complete | `app.rs::indicator_defers_until_the_threshold` | Direct, including clear-on-snapshot. |
| Selection survives swap identity-first; fallback nearest sibling, never reset to top | `app.rs::file_selection_survives_a_snapshot_swap_by_identity`, `a_vanished_selection_falls_to_the_nearest_sibling`, `changelist_selection_survives_by_name_else_nearest`, `commit_selection_survives_by_id_else_clamps`, `hunk_selection_survives_a_snapshot_swap_by_content`, `a_vanished_hunk_selection_clamps_and_an_empty_file_exits_hunk_mode` | Thorough. |
| Stale-action race: stage/assign validate at apply, fail soft, then refresh | `tests/staging.rs::a_stale_hunk_fails_soft_with_a_notice_and_no_index_write`, `a_stale_hunk_fails_soft_on_unstage_too`; `tests/assign.rs::a_stale_hunk_fails_soft_and_the_fresh_one_is_still_assigned` | Core soft-fail direct; the "immediate refresh after" is lib.rs wiring — untested. |
| Commit keeps its stricter refresh-gate | `tests/commit.rs::payload_drift_returns_to_the_confirm_step` | See ADR 0004 table. |

## ADR 0006 — Crate architecture: deep core, two thin frontends

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Three crates: core lib, tui lib, bin | Compile-time structure | Workspace has `gitchange-core`, `gitchange-tui`, `gitchange` (+ dev-only `xtask`). |
| git2 appears only in core's Cargo.toml; frontends reach git only through core | Compile-time structure — verified | `git2` only in `crates/gitchange-core/Cargo.toml`; zero `git2` references in tui/bin source. No CI lint guards regression, but a frontend `git2::` use wouldn't compile without adding the dep. |
| TUI exposed to bin as essentially `run() -> Result` | Compile-time structure | `gitchange-tui/src/lib.rs::run`. |
| Sync-operations layer (CLI: one blocking refresh per invocation) | `crates/gitchange/tests/cli.rs::status_lists_changed_files_and_exits_0` and siblings | CLI integration tests drive the whole sync path through the real binary. |
| `Engine` lives in core, layered on sync ops, events over crossbeam channel | `gitchange-core/src/engine.rs` unit tests + `tests/engine.rs::touching_a_file_produces_a_refresh_complete` | Structural + behavioural. |
| No async runtime — threads and channels only | Compile-time structure — verified | No async runtime anywhere in workspace Cargo.tomls. |
| Core's error type: single `thiserror` enum, variants carved by caller action; core never uses `anyhow` | Structural + `tests/commit.rs::hook_rejection_changes_nothing`, `tests/changelists.rs::a_held_lockfile_fails_fast_with_lock_contention`, `tests/refresh.rs::a_non_utf8_path_fails_refresh_loudly_never_lossily`, `tests/staging.rs::a_refused_apply_reports_apply_failed_and_stages_nothing` | Zero `anyhow` in core. The recorded doc drift here — ADR named an `ApplyFailed` variant the enum lacked — closed in 6487c4b: the variant exists, is a hard error rather than a soft notice, and is now covered. |
| `git2::Error` never in core's public interface | Compile-time/structural | `Error::Backend(Box<dyn std::error::Error…>)` wraps opaquely. |
| Engine degradation is an event, not an error | `src/engine.rs::watcher_death_degrades_and_polling_keeps_refreshing` | Ships as `ConditionStarted(Condition::WatcherDegraded)` — same contract, evolved shape (see ADR 0007 vocabulary). |
| Bin maps error variants to exit codes; TUI matches variants into presentation | `crates/gitchange/tests/cli.rs::switch_to_unknown_name_exits_1_with_message_on_stderr`, `switch_without_a_name_exits_2`, `status_outside_a_repo_exits_1_with_message_on_stderr`, `unknown_subcommand_exits_2`, `bare_invocation_outside_a_repo_exits_1`; TUI: `app.rs::hook_rejection_modal_lands_over_the_restored_dialog`, `the_error_modal_swallows_keys_scrolls_and_dismisses` | Exit-code contract directly tested. |
| One binary: bare invocation = TUI, subcommands = CLI | Partial: `cli.rs::bare_invocation_outside_a_repo_exits_1` proves bare invocation takes the TUI path; happy-path TUI launch untestable in CI | Reasonable. |

## ADR 0007 — Presentation channels & git-state guards

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Three severities with distinct glyphs `·`/`!`/`✗` in the Log panel | `crates/gitchange-tui/tests/render.rs::log_entries_render_with_their_severity_glyphs` | Glyph-per-severity asserted on the rendered buffer. |
| Log stream carries executed-git-op echoes (transparency channel) | **GAP** (partial) | The echoes exist in `gitchange-tui/src/lib.rs`'s op executor, which has no tests; no test asserts a command echo lands in the log. |
| Pins appear on condition start and self-clear on end; known pins: watcher degraded, op-in-progress (with conflicted count), detached HEAD | `app.rs::conditions_pin_and_self_clear`; rendering: `render.rs::pins_render_as_a_banner_atop_the_log_stream` | All three pin texts asserted, incl. banner-above-stream ordering. |
| Pins never manually dismissable | Indirect — `app.rs::conditions_pin_and_self_clear` | Negative property; acceptable. |
| Notice severity: ambiguous/auto-capture | `matcher.rs::a_routine_auto_capture_notices_once`, `a_hunk_overlapping_two_changelists_captures_to_active_with_a_notice`; TUI mapping: `app.rs::notices_land_in_the_log_at_notice_severity` | Direct. |
| Notice severity: dormant revival | `matcher.rs::dormant_revival_notices_with_a_per_changelist_count` | Per-changelist count asserted. |
| Changed-return hunks don't revive; they auto-capture instead (with its own notice) | Partial — `matcher.rs::dormant_records_never_revive_via_overlap`, `binary_dormant_revival_is_exact_only` | Non-revival + capture asserted; neither test asserts the accompanying AutoCaptured notice in that scenario. |
| Info: going-dormant and 14-day prune events | **Recorded deviation** (#34) | Core emits no notice for either. Prune behaviour itself is tested (`matcher.rs::dormant_records_prune_after_fourteen_days`), just not an info event. |
| Info: soft no-op `c` on the all view | `app.rs::c_on_all_is_a_polite_noop_and_opens_the_flow_scoped` | Info log line asserted. |
| Info: soft no-op `space` on a conflicted file | `app.rs::conflicted_files_group_first_and_space_refuses` | Refusal + info severity + text asserted. |
| Info: watcher recovery | **GAP** (partial) | `"watcher recovered"` is pushed at info but no test asserts the log line — only the pin clearing is tested. |
| Error: every modal also logged at `✗` | `app.rs::a_failed_refresh_keeps_the_last_snapshot_and_modals` | **Recorded deviation** (#34): log line is `{title} — first line of detail`; full detail doesn't survive dismissal. |
| Error modal: title names operation, detail verbatim & scrollable, `esc`/`enter` dismiss, swallows keys, no inline actions | `app.rs::the_error_modal_swallows_keys_scrolls_and_dismisses`; `render.rs::the_error_modal_renders_the_detail_verbatim` | Covered across the two tests. |
| Hook rejection returns to commit dialog with message intact; rejection modal on top; hook stderr verbatim | Core: `commit.rs::hook_rejection_changes_nothing`, `a_hook_sees_the_commits_true_content`; TUI: `app.rs::hook_rejection_modal_lands_over_the_restored_dialog` | Partial on wiring: the `Err(HookRejected)` routing in `lib.rs::run_commit` is untested. `ctrl+n` toggle covered by `app.rs::dialog_edits_message_body_and_flags_then_commits`. |
| Lockfile contention: plain modal, no retry loop | Core: `changelists.rs::a_held_lockfile_fails_fast_with_lock_contention`; engine: `src/engine.rs::lock_contention_is_retried_not_surfaced` | No TUI test asserts the modal specifically (rides the generic `show_error` path). |
| Unmerged paths quarantined per-file, excluded from hunk universe, regardless of records | `conflicts.rs::an_unmerged_path_is_quarantined_from_the_universe`; binary precedence: `hunk_universe.rs::a_conflicted_binary_stays_quarantined` | Also asserts a conflicted file never falls into "unassigned". |
| Conflicts group rendered first, error-tinted, own glyph | `render.rs::the_conflicts_group_renders_first_without_stage_marks`; `app.rs::conflicted_files_group_first_and_space_refuses` | Ordering, `U` sigil, no stage mark asserted; the error *tint* colour is not. |
| Conflicts group shrinks live as files are resolved | `conflicts.rs::an_unmerged_path_is_quarantined_from_the_universe` | Mid-merge resolution re-enters the live universe while the guard holds. |
| Conflicted files' records freeze — no matching, no dormancy clock | `conflicts.rs::quarantine_freezes_records_and_resolution_relands_them` | Records byte-identical across conflicted refreshes. |
| Frozen records re-enter normal matching (exact *then overlap*) on resolution | Partial — same test | Only the exact-anchor re-land is tested; overlap-inheritance after merge-shifted resolution content is not. |
| Commit globally guarded during any git operation (merge, rebase ×2, cherry-pick, revert, `git am`) | `conflicts.rs::a_merge_in_progress_is_reported_and_guards_commit`; the rest in `operations.rs` — `every_rebase_backend_reports_rebase_and_guards_commit`, `a_cherry_pick_reports_cherry_pick_and_guards_commit`, `a_cherry_pick_sequence_…`, `a_revert_reports_revert_and_guards_commit`, `a_revert_sequence_…`, `an_am_in_progress_reports_am_and_guards_commit` (#57) | Every operation is driven by real git in a temp repo, each test asserting the `RepositoryState` it reached before the guard (ADR 0008's fixture rule amended for the shell-out). Two mapping arms stay untested for want of a real-git producer: `RebaseMerge` (git 2.49 marks `--merge` interactive, so both merge-backend flavours land on `RebaseInteractive`) and `ApplyMailboxOrRebase`. |
| Guard UX: `c` is a no-op with the info line | `app.rs::the_operation_guard_makes_c_a_soft_noop` | Exact text and severity asserted. |
| Staging never guarded beyond unmerged paths | `conflicts.rs::a_merge_in_progress_is_reported_and_guards_commit`; `app.rs::the_operation_guard_makes_c_a_soft_noop`; every `operations.rs` test (#57) | Clean file stages mid-merge at both layers, and mid-rebase/cherry-pick/revert/am in core — asserted by hunk against real index content. |
| Detached HEAD: pin, no commit guard | Partial — pin: `app.rs::conditions_pin_and_self_clear`; reporting: `refresh.rs::snapshot_head_reports_detached_by_short_id` | No test commits while detached to prove the *absence* of a guard. |
| Unborn branch: commit allowed, diffs against empty tree | `commit.rs::unborn_branch_initial_commit_works`; `hunk_universe.rs::an_unborn_head_diffs_against_the_empty_tree` | Both halves covered. |
| Commit dialog restorable after failed commit (message preserved until success) | `app.rs::a_failed_commit_restores_the_dialog_and_success_closes_it`, `drift_reconfirms_with_the_message_kept`, `a_stale_payload_routes_through_the_warn_overlay` | Draft equality asserted; success closes with no toast. |
| ConditionStarted/ConditionEnded event vocabulary | `src/engine.rs::watcher_death_degrades_and_polling_keeps_refreshing` (ConditionStarted only) | **Recorded deviation** (#34): Condition stays watcher-only; `ConditionEnded` never emitted by the Engine in v0.1. |
| No message line / no toast / no unseen-notice indicator | Indirect — `app.rs::a_failed_commit_restores_the_dialog_and_success_closes_it` | Architectural negatives. |

## ADR 0008 — Testing method (verified as method)

| Mandated method | Followed? / evidence |
|---|---|
| Test through core's public sync ops against real temp-dir repos; no fake `GitBackend` | Yes — every integration test drives `Repo::discover` on a `RepoFixture`; the only `impl GitBackend` is `git2_backend.rs`. |
| Shared programmatic `RepoFixture` builder with `with_hook()` | Yes — `tests/support/mod.rs`; `with_hook` exercised by `commit.rs::hook_rejection_changes_nothing`. |
| No checked-in fixture repos | Yes. |
| Apply-correctness corpus, data-driven, seeded from the risk list | Yes — `tests/apply_corpus.rs`, 42 cases across all ADR-named risk categories (trailing newline, CRLF, blank-line-only, pure deletions, adjacent hunks, create/delete, empty-file edges, mode changes `#[cfg(unix)]`, non-UTF-8 content, identical repeated hunks). |
| Engine decision logic unit-tested deterministically via injectable event source | Yes — all three mandated domain rules (debounce, self-loop, last-request-wins) covered synthetically in `src/engine.rs`. |
| One real-fs watcher smoke test per CI OS | Yes — `tests/engine.rs::touching_a_file_produces_a_refresh_complete`, full OS matrix. |
| No assertions on real debounce timing | Yes — generous ceilings/synthetic config only. |
| CI: Linux + macOS + Windows on stable Rust; no git version matrix; hooks per-test not CI setup | Yes — `.github/workflows/ci.yml`. |

Method deviation: none found. (Mode-change corpus cases are `#[cfg(unix)]`, consistent with the ADR's own "filemode is off on Windows" note.)

## ADR 0009 — Binary files: whole-file degenerate hunks

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Changed binary = one degenerate whole-file hunk with a normal membership record | `hunk_universe.rs::a_changed_binary_file_is_one_whole_file_hunk` | 1 hunk, empty lines, OID anchor, sizes all asserted. |
| New binary change → active changelist (with auto-capture notice) | `matcher.rs::a_reexported_binary_keeps_its_changelist_via_path_continuity` (first phase) | AutoCaptured notice asserted. |
| Unassigned when no changelists exist | Partial — text-only assertion (`with_no_changelists_hunks_are_unassigned_and_no_state_file_is_written`) | Rule is uniform in code; minor. |
| Assignable between changelists like any hunk | `matcher.rs::a_binary_whole_file_hunk_is_assignable_like_any_other` | Via the real `assign_hunks` op. |
| Anchor = blob-OID pair; one-sided for add/delete | `hunk_universe.rs::a_changed_binary_file_is_one_whole_file_hunk`, `added_and_deleted_binaries_have_one_sided_anchors` | Direct. |
| Tier 1 exact: changed-side OID match holds membership | `matcher.rs::an_unchanged_binary_matches_exactly_after_a_move` | Also asserts no notice. |
| Tier 2 path continuity: re-export keeps membership, anchor updates | `matcher.rs::a_reexported_binary_keeps_its_changelist_via_path_continuity` | Anchor mutation asserted in `state.json`. |
| Dormant revival exact only (path + OID); different content = fresh capture | `matcher.rs::binary_dormant_revival_is_exact_only` | Direct. |
| `space` on unstaged binary = whole-file index write; unstage = HEAD blob back / entry dropped | `staging.rs::staging_a_binary_whole_file_hunk_is_a_whole_file_index_write`, `staging_an_untracked_binary_and_unstaging_drops_the_entry` | Index bytes asserted directly. |
| `◐` unreachable via derivation | Indirect — `hunk_universe.rs::binary_staging_is_derived_by_oid_compare` | Adequate given the counting design. |
| `◑` derived by OID compare, both flavours | `hunk_universe.rs::binary_staging_is_derived_by_oid_compare` | Both flavours incl. `index_only`. |
| `space` on `◑` sets index := worktree | Core: `staging.rs::space_on_a_stale_binary_restages_the_worktree_blob`; TUI: `app.rs::space_on_a_stale_binary_restages_the_file` | Direct. |
| Warn-and-confirm + temp-index commit unchanged; drift guard includes OID | `commit.rs::a_changelist_containing_a_binary_commits_it_whole`, `a_stale_binary_warns_and_commits_the_staged_blob`, `a_staged_binary_deletion_commits_the_removal`, `a_restaged_binary_blob_drifts_the_confirmation`, `a_binary_worktree_over_staged_text_commits_the_staged_text` | Strong, incl. the mixed text/binary edge. |
| Diff placeholder `Binary file changed (X → Y)` + single-size variants | `app.rs::binary_diff_placeholder_shows_sizes_per_variant` | All three variants, exact strings. |
| No new glyphs; file rows derive `●○` normally | Indirect | No render test on a binary row specifically; minor. |
| Hunk-mode entry on binary = polite no-op, no log event | `app.rs::enter_on_a_binary_is_a_polite_no_op` | Direct. |
| `state.json` OID record shape; dormancy + 14-day prune unchanged | Partial — OID shape and dormancy direct; 14-day prune only tested with a text-anchor record | Minor. |
| Conflicted binary never derives a whole-file hunk | `hunk_universe.rs::a_conflicted_binary_stays_quarantined` | Direct. |

## ADR 0010 — Path encoding: UTF-8 strings, loud failure

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Refresh in a repo containing a non-UTF-8 path fails with `Error::NonUtf8Path` — loud, never lossy | `crates/gitchange-core/tests/refresh.rs::a_non_utf8_path_fails_refresh_loudly_never_lossily` | Solid on the error variant; exercises one surface (staged blob at a raw path). Other surfaces funnel through the same `utf8_path` helper. |
| The error names the offending path | Partial — same test | Only `matches!(err, Error::NonUtf8Path { .. })` asserted; the `path` field's content never checked. |
| Nothing is persisted from the failed refresh | **GAP** | No test creates prior state, triggers `NonUtf8Path`, and checks the state file is untouched. |
| All paths are UTF-8 `String`s; comparison is plain `String` equality on git's bytes | By construction (type system) | Adjacent: `tests/staging.rs::staging_a_hunk_of_a_non_utf8_text_file_keeps_the_bytes_verbatim` covers non-UTF-8 *content*, a different concern. |

## ADR 0011 — Renames: presented as delete + add, detection off

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| A rename presents as `Deleted` at old path + `Untracked` at new path; no `Renamed` kind | `crates/gitchange-core/tests/hunk_universe.rs::a_rename_presents_as_delete_plus_untracked` | Direct; `find_similar` appears nowhere, and `ChangeKind` has no `Renamed` variant (compile-time). |
| A rename orphans the old path's records into dormancy; new path's hunks are fresh changes assigned to active ("visible-not-silent") | **GAP** | No test puts a record on a file, renames it, and asserts old-record dormancy + new-path active-capture. Adjacent-only: dormancy via content vanishing. |
| Same orphan-to-dormancy for binary whole-file hunks across a rename | **GAP** | Path continuity is tested at the *same* path only. |
| Staging + committing both halves still yields a rename in git's log-side detection | GAP (arguably out of scope) | A property of git itself, not gitchange code. |

## ADR 0012 — HEAD moves: commutation + dormancy guard

| Mandated behaviour | Covering test (file::test_name) | Notes |
|---|---|---|
| Own commit shifts surviving same-file records by committed deltas; later anchor-broken edit still inherits via tier-2 | `crates/gitchange-core/tests/commit.rs::committing_one_changelist_commutes_same_file_records` | Direct; third active changelist so re-attachment can't pass by luck. |
| Own commit rewrites retained `◑` records against new HEAD | `commit.rs::a_residual_stale_hunk_reattaches_after_an_own_commit`, `a_residual_stale_hunk_reattaches_when_the_payload_shifts_it` | Both flavours pinned. |
| Own commit stamps the new baseline HEAD in the same locked update | `commit.rs::commit_stamps_the_baseline_in_the_same_update` | Observable contract asserted. |
| External move: tier-2 disabled on moved paths — stale live records go dormant, anchor-broken hunks capture to active | `head_moves.rs::a_shifted_neighbour_goes_dormant_loudly_instead_of_misfiling`, `a_shifted_neighbour_clear_of_stale_records_captures_to_active` | Both flavours; owners, notice content, dormant set asserted. |
| Per-path notice, fired only when an outcome actually changed, naming affected changelists | notice asserts across the four dormancy tests + `an_untouched_neighbour_survives_an_external_partial_commit` + `a_head_move_touching_only_other_paths_leaves_tier_two_intact` | Loss-only firing covered from both sides. |
| Tier-1 exact anchors still rescue untouched hunks across an external move | `head_moves.rs::an_untouched_neighbour_survives_an_external_partial_commit` | Direct. |
| Guard is path-scoped; the persisting refresh advances the baseline | `head_moves.rs::a_head_move_touching_only_other_paths_leaves_tier_two_intact` | Direct. |
| Unresolvable old baseline (gc'd) degrades to all-paths-affected | `head_moves.rs::an_unresolvable_baseline_degrades_to_all_paths_affected` | Direct. |
| Missing baseline (pre-schema file) adopts current HEAD once, silently | `head_moves.rs::a_pre_baseline_state_file_adopts_the_head_move_silently` | Direct. |
| Retained `◑` records across external moves are exact-revival-only | `head_moves.rs::a_residual_staged_stale_hunk_goes_dormant_across_an_external_commit`, `a_residual_staged_stale_hunk_sheds_membership_when_the_commit_shifts_it`; `matcher.rs::dormant_records_never_revive_via_overlap` | Composite but each piece directly asserted. |
| Matcher purity: affected-path set enters as explicit input; HEAD identity never enters matcher | By construction — `src/matcher.rs::run` takes `affected: &AffectedPaths` | Exercised end-to-end by all head_moves tests. |
| Notice rendered like `AmbiguousOverlap` by CLI and the Log panel | **GAP** | Rendering code exists but no CLI or TUI test triggers or renders `HeadMoveDormancy`. |
| ADR 0010 carve-out: non-UTF-8 path in the baseline↔HEAD tree diff is skipped, not a loud failure | **GAP** | Implemented (`src/git2_backend.rs`, with an ADR comment), but no test constructs such a tree diff and asserts refresh succeeds. |
