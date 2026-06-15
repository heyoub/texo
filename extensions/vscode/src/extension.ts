import * as vscode from "vscode";
import { runCheckStaleness } from "./runner";
import { publishDiagnostics } from "./diagnostics";

export function activate(context: vscode.ExtensionContext): void {
  const checkCurrent = vscode.commands.registerCommand("texo.checkCurrentFile", async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      return;
    }
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
    const report = await runCheckStaleness(root, editor.document.uri.fsPath);
    publishDiagnostics(report);
  });

  const checkWorkspace = vscode.commands.registerCommand("texo.checkWorkspace", async () => {
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root) {
      return;
    }
    const report = await runCheckStaleness(root, root);
    publishDiagnostics(report);
  });

  const onSave = vscode.workspace.onDidSaveTextDocument(async (doc) => {
    const cfg = vscode.workspace.getConfiguration("texo");
    if (!cfg.get<boolean>("checkOnSave") || doc.languageId !== "markdown") {
      return;
    }
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
    const report = await runCheckStaleness(root, doc.uri.fsPath);
    publishDiagnostics(report);
  });

  context.subscriptions.push(checkCurrent, checkWorkspace, onSave);
}

export function deactivate(): void {}
