# Conventions

- Product name: **texo**
- Crates: `texo-core`, `texo-cli`, `texo-mcp`, `texo-semantics` (optional ML backends), `texo-extract` (LLM extractor binary), `texo-agent` (memory-agent HTTP server)
- Config/store: `.texo/config.toml`, `.texo/store/`
- Use newtypes for IDs and sequences in domain code
- Use `IngestMode` not bare `bool` for dry-run vs commit
- Replay mutations go through lifecycle helpers, not raw status assignment
- Integration tests use real BatPak stores — no mocks
