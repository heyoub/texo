import * as path from "node:path";
import * as vscode from "vscode";
import { runCheckStaleness, runAgentContext } from "./runner";
import { publishDiagnostics } from "./diagnostics";

export function activate(context: vscode.ExtensionContext): void {
  const checkCurrent = vscode.commands.registerCommand("texo.checkCurrentFile", async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor) {
      return;
    }
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
    const report = await runCheckStaleness(root, editor.document.uri.fsPath);
    publishDiagnostics(report, root);
  });

  const checkWorkspace = vscode.commands.registerCommand("texo.checkWorkspace", async () => {
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
    if (!root) {
      return;
    }
    const report = await runCheckStaleness(root, root);
    publishDiagnostics(report, root);
  });

  const generateAgentContext = vscode.commands.registerCommand(
    "texo.generateAgentContext",
    async () => {
      const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath;
      if (!root) {
        return;
      }
      const outPath = path.join(root, "agent-context.json");
      try {
        await runAgentContext(root, outPath);
        vscode.window.showInformationMessage(`texo: wrote agent context to ${outPath}`);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        vscode.window.showErrorMessage(`texo: failed to generate agent context. ${message}`);
      }
    },
  );

  const onSave = vscode.workspace.onDidSaveTextDocument(async (doc) => {
    const cfg = vscode.workspace.getConfiguration("texo");
    if (!cfg.get<boolean>("checkOnSave") || doc.languageId !== "markdown") {
      return;
    }
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
    const report = await runCheckStaleness(root, doc.uri.fsPath);
    publishDiagnostics(report, root);
  });

  const onOpen = vscode.workspace.onDidOpenTextDocument(async (doc) => {
    const cfg = vscode.workspace.getConfiguration("texo");
    if (!cfg.get<boolean>("checkOnOpen") || doc.languageId !== "markdown") {
      return;
    }
    const root = vscode.workspace.workspaceFolders?.[0]?.uri.fsPath ?? ".";
    const report = await runCheckStaleness(root, doc.uri.fsPath);
    publishDiagnostics(report, root);
  });

  context.subscriptions.push(checkCurrent, checkWorkspace, generateAgentContext, onSave, onOpen);
}

export function deactivate(): void {}
