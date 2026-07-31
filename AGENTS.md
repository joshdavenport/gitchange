## Agent skills

### Issue tracker

Issues live in this repo's GitHub Issues (`gh` CLI). The wayfinder map for the initial-implementation effort is [issue #20](https://github.com/joshdavenport/gitchange/issues/20). See `docs/agents/issue-tracker.md`.

### Triage labels

Default canonical labels (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Manual-testing sandboxes

`cargo xtask sandbox make --all` builds resettable scenario repos under `.sandbox/` for eyeballing the TUI. See `docs/agents/sandbox.md`.

### Benchmark harness

`cargo xbench` times the real RefreshJob over synthetic repos at graduated scales and reports scaling shape per dimension — the measurement gate for all ADR 0005 mitigations. See `docs/agents/benchmarks.md`.

### Domain docs

Single-context: `CONTEXT.md` glossary at the repo root, ADRs in `docs/adr/`. See `docs/agents/domain.md`.
