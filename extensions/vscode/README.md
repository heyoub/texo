# texo VS Code extension

Stale markdown diagnostics powered by the [`texo`](https://github.com/heyoub/texo) CLI claim-chain.

## Requirements

- [`texo`](https://github.com/heyoub/texo) CLI on your `PATH`, or set `texo.binaryPath`
- Run `texo init --workspace demo` and `texo ingest sample_sources` in your repo root

## Usage

- **Check on save** (default): flags stale claims in markdown files
- Command palette: `texo: Check Current File`, `texo: Check Workspace`, `texo: Generate Agent Context`

## Settings

| Setting | Default | Description |
|---|---|---|
| `texo.binaryPath` | `texo` | Path to CLI binary |
| `texo.workspaceId` | `demo` | BatPak workspace scope (matches `--workspace`) |
| `texo.checkOnSave` | `true` | Check markdown on save |
| `texo.checkOnOpen` | `false` | Check markdown on open |
| `texo.checkTimeoutMs` | `30000` | Kill CLI invocations that exceed this many milliseconds |

Save-triggered checks are debounced per file (trailing edge, 400ms); a newer
save cancels a superseded in-flight check. The status bar shows a `texo` item
while a check runs (spinner) and after a failure (warning; details in the
`texo` output channel). If `.texo/config.toml` is missing, checks are skipped
and the extension says so once per session.

## Package locally

```sh
just ext-package
```

Produces `extensions/vscode/texo-*.vsix`.

## Publish (one-time)

```sh
npm install -g @vscode/vsce
vsce login texo
vsce publish
```

Publisher account required; not automated in CI.
