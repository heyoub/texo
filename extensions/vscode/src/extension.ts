import * as fs from "node:fs";
import * as path from "node:path";
import * as vscode from "vscode";
import { runCheckStaleness, runAgentContext, TexoTimeoutError, isAbortError } from "./runner";
import { createDiagnosticCollection, publishDiagnostics } from "./diagnostics";

/** Trailing-edge debounce window for save/open-triggered checks, per file. */
const DEBOUNCE_MS = 400;

const NOT_INITIALIZED_MESSAGE =
  "texo is not initialized in this workspace (no .texo/config.toml). " +
  "Run `texo init` to enable staleness checks.";

function reportError(err: unknown): void {
  const message = err instanceof Error ? err.message : String(err);
  if (err instanceof TexoTimeoutError) {
    void vscode.window.showWarningMessage(`texo: ${message}`);
    return;
  }
  void vscode.window.showErrorMessage(`texo: ${message}`);
}

/** Run an async handler, surfacing any rejection instead of leaving a floating promise. */
function guard(handler: () => Promise<void>): () => Promise<void> {
  return async () => {
    try {
      await handler();
    } catch (err) {
      reportError(err);
    }
  };
}

/** True when `.texo/config.toml` exists under `root`, i.e. `texo init` has run. */
function isInitialized(root: string): boolean {
  return fs.existsSync(path.join(root, ".texo", "config.toml"));
}

interface StatusIndicator extends vscode.Disposable {
  /** A check started: show the spinner. */
  begin(): void;
  /** A check finished cleanly: back to idle, clearing any error state. */
  succeed(): void;
  /** A check failed: show the warning state with `message` in the tooltip. */
  fail(message: string): void;
  /** A check was superseded and aborted: back off without touching error state. */
  cancel(): void;
}

/** Status bar item reflecting idle / checking / error across concurrent checks. */
function createStatusIndicator(): StatusIndicator {
  const item = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Left);
  item.name = "texo";
  let active = 0;
  let lastError: string | undefined;

  const render = (): void => {
    if (active > 0) {
      item.text = "$(sync~spin) texo";
      item.tooltip = "texo: checking…";
    } else if (lastError !== undefined) {
      item.text = "$(warning) texo";
      item.tooltip = `texo: last check failed — ${lastError}`;
    } else {
      item.text = "texo";
      item.tooltip = "texo: idle";
    }
  };
  render();
  item.show();

  const settle = (error: string | undefined, keepError: boolean): void => {
    active = Math.max(0, active - 1);
    if (!keepError) {
      lastError = error;
    }
    render();
  };

  return {
    begin(): void {
      active += 1;
      render();
    },
    succeed(): void {
      settle(undefined, false);
    },
    fail(message: string): void {
      settle(message, false);
    },
    cancel(): void {
      settle(undefined, true);
    },
    dispose(): void {
      item.dispose();
    },
  };
}

export function activate(context: vscode.ExtensionContext): void {
  const collection = createDiagnosticCollection();
  const status = createStatusIndicator();
  const output = vscode.window.createOutputChannel("texo");
  context.subscriptions.push(collection, status, output);

  /** Wrap a CLI run so the status bar tracks it; errors still propagate. */
  async function withStatus<T>(run: () => Promise<T>): Promise<T> {
    status.begin();
    try {
      const result = await run();
      status.succeed();
      return result;
    } catch (err) {
      status.fail(err instanceof Error ? err.message : String(err));
      throw err;
    }
  }

  // --- Missing-config manners: explain once per session, then stay quiet. ---

  let warnedNotInitialized = false;

  /** For automatic (save/open) checks: skip quietly, explaining once per session. */
  function noteNotInitialized(root: string): void {
    if (warnedNotInitialized) {
      return;
    }
    warnedNotInitialized = true;
    output.appendLine(
      `texo: no .texo/config.toml found in ${root} — skipping checks until \`texo init\` is run.`,
    );
    void vscode.window.showInformationMessage(NOT_INITIALIZED_MESSAGE);
  }

  // --- Debounced check-on-save/open: one trailing-edge run per file. ---

  const pendingTimers = new Map<string, ReturnType<typeof setTimeout>>();
  const inFlight = new Map<string, AbortController>();

  /** Run a scoped check for one file; quiet on failure (output channel + status bar). */
  async function runScheduledCheck(root: string, target: string): Promise<void> {
    // A newer save supersedes any run still in flight for this file.
    inFlight.get(target)?.abort();
    const controller = new AbortController();
    inFlight.set(target, controller);
    status.begin();
    try {
      const report = await runCheckStaleness(root, target, controller.signal);
      publishDiagnostics(collection, report, root, [target]);
      status.succeed();
    } catch (err) {
      if (isAbortError(err)) {
        status.cancel();
      } else {
        const message = err instanceof Error ? err.message : String(err);
        output.appendLine(`texo: check failed for ${target} — ${message}`);
        status.fail(message);
      }
    } finally {
      if (inFlight.get(target) === controller) {
        inFlight.delete(target);
      }
    }
  }

  function scheduleCheck(doc: vscode.TextDocument): void {
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
    if (!isInitialized(root)) {
      noteNotInitialized(root);
      return;
    }
    const target = doc.uri.fsPath;
    const previous = pendingTimers.get(target);
    if (previous !== undefined) {
      clearTimeout(previous);
    }
    const timer = setTimeout(() => {
      pendingTimers.delete(target);
      void runScheduledCheck(root, target);
    }, DEBOUNCE_MS);
    pendingTimers.set(target, timer);
  }

  const scheduler: vscode.Disposable = {
    dispose: (): void => {
      for (const timer of pendingTimers.values()) {
        clearTimeout(timer);
      }
      pendingTimers.clear();
      for (const controller of inFlight.values()) {
        controller.abort();
      }
      inFlight.clear();
    },
  };
  context.subscriptions.push(scheduler);

  // --- Commands (explicit user actions: loud feedback is fine). ---

  const checkCurrent = vscode.commands.registerCommand(
    "texo.checkCurrentFile",
    guard(async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        return;
      }
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
      if (!isInitialized(root)) {
        void vscode.window.showInformationMessage(NOT_INITIALIZED_MESSAGE);
        return;
      }
      const target = editor.document.uri.fsPath;
      const report = await withStatus(() => runCheckStaleness(root, target));
      publishDiagnostics(collection, report, root, [target]);
    }),
  );

  const checkWorkspace = vscode.commands.registerCommand(
    "texo.checkWorkspace",
    guard(async () => {
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      if (!root) {
        return;
      }
      if (!isInitialized(root)) {
        void vscode.window.showInformationMessage(NOT_INITIALIZED_MESSAGE);
        return;
      }
      const report = await withStatus(() => runCheckStaleness(root, root));
      publishDiagnostics(collection, report, root);
    }),
  );

  const generateAgentContext = vscode.commands.registerCommand(
    "texo.generateAgentContext",
    guard(async () => {
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      if (!root) {
        return;
      }
      if (!isInitialized(root)) {
        void vscode.window.showInformationMessage(NOT_INITIALIZED_MESSAGE);
        return;
      }
      const outPath = path.join(root, "agent-context.json");
      await withStatus(() => runAgentContext(root, outPath));
      void vscode.window.showInformationMessage(`texo: wrote agent context to ${outPath}`);
    }),
  );

  const onSave = vscode.workspace.onDidSaveTextDocument((doc) => {
    const cfg = vscode.workspace.getConfiguration("texo");
    if (!cfg.get<boolean>("checkOnSave") || doc.languageId !== "markdown") {
      return;
    }
    scheduleCheck(doc);
  });

  const onOpen = vscode.workspace.onDidOpenTextDocument((doc) => {
    const cfg = vscode.workspace.getConfiguration("texo");
    if (!cfg.get<boolean>("checkOnOpen") || doc.languageId !== "markdown") {
      return;
    }
    scheduleCheck(doc);
  });

  context.subscriptions.push(checkCurrent, checkWorkspace, generateAgentContext, onSave, onOpen);
}

export function deactivate(): void {}
