import * as path from "node:path";
import * as vscode from "vscode";
import { runCheckStaleness, runAgentContext } from "./runner";
import { createDiagnosticCollection, publishDiagnostics } from "./diagnostics";

function reportError(err: unknown): void {
  const message = err instanceof Error ? err.message : String(err);
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

export function activate(context: vscode.ExtensionContext): void {
  const collection = createDiagnosticCollection();
  context.subscriptions.push(collection);

  const checkCurrent = vscode.commands.registerCommand(
    "texo.checkCurrentFile",
    guard(async () => {
      const editor = vscode.window.activeTextEditor;
      if (!editor) {
        return;
      }
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
      const target = editor.document.uri.fsPath;
      const report = await runCheckStaleness(root, target);
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
      const report = await runCheckStaleness(root, root);
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
      const outPath = path.join(root, "agent-context.json");
      await runAgentContext(root, outPath);
      void vscode.window.showInformationMessage(`texo: wrote agent context to ${outPath}`);
    }),
  );

  const onSave = vscode.workspace.onDidSaveTextDocument((doc) => {
    void guard(async () => {
      const cfg = vscode.workspace.getConfiguration("texo");
      if (!cfg.get<boolean>("checkOnSave") || doc.languageId !== "markdown") {
        return;
      }
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
      const target = doc.uri.fsPath;
      const report = await runCheckStaleness(root, target);
      publishDiagnostics(collection, report, root, [target]);
    })();
  });

  const onOpen = vscode.workspace.onDidOpenTextDocument((doc) => {
    void guard(async () => {
      const cfg = vscode.workspace.getConfiguration("texo");
      if (!cfg.get<boolean>("checkOnOpen") || doc.languageId !== "markdown") {
        return;
      }
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
      const target = doc.uri.fsPath;
      const report = await runCheckStaleness(root, target);
      publishDiagnostics(collection, report, root, [target]);
    })();
  });

  context.subscriptions.push(checkCurrent, checkWorkspace, generateAgentContext, onSave, onOpen);
}

export function deactivate(): void {}
