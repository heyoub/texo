# Conventions

Supersedes: see [ADR-003](ADR-003-single-crate-rebuild.md).

- Product name: `texo`.
- Crate name and binary name: `texo`.
- Config/store: `.texo/config.toml` plus per-workspace store paths.
- Commands use `cargo run --bin texo -- ...`.
- The lint bar is BatPak-grade: `cargo clippy --all-targets --all-features --
  -D warnings` with denies for unwrap, panic, todo, unimplemented, dbg,
  unsanctioned prints, lossy casts, and disallowed runtime/process helpers.
- Use `expect("reason")` only when the invariant is already proven locally.
- Every public `Result` function documents `# Errors`.
- Transport boundaries map failures into typed `TexoError` variants with stable
  `.code()` tokens.
- CLI exit status is tristate: `0` complete, `1` failed or findings-only, and
  `2` partial with committed usable work. `check-staleness` findings remain `1`.
- CLI/stdio/HTTP surface prints are explicit output contracts and carry local
  `#[expect(clippy::print_stdout|print_stderr, reason = "...")]` annotations.
- BatPak appends go through `TexoEffectBackend`; receipts verify before being
  returned to callers.
- Session turns stay in non-zero lanes until transcript ingest records lane-0
  source/claim events.
- Agent integration is declarative and project-local: managed markers and
  structural JSON/TOML merges only. Hooks invoke fixed read-only Texo commands;
  workspace configuration never supplies executable hook commands.
- Backups carry journal authority and config only. Caches, warm projections,
  generated views, and client adapters are rebuilt rather than restored as
  source truth. Restore accepts only a fresh root, stages privately beside it,
  rechecks every copied digest and the BatPak chain, then atomically publishes.
- Backup evidence is self-consistency evidence unless its printed manifest
  hash is pinned outside the backup and supplied during verification; it is
  never described as a signature or independent proof of authenticity.
  Supply the out-of-band hash to restore when authenticity, rather than backup
  self-consistency, is required.
