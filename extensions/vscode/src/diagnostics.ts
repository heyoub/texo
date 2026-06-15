import * as vscode from "vscode";
import { lineRange } from "./range";
import { StalenessReport } from "./runner";

const collection = vscode.languages.createDiagnosticCollection("texo");

export function publishDiagnostics(report: StalenessReport): void {
  const byFile = new Map<string, vscode.Diagnostic[]>();

  for (const diag of report.diagnostics) {
    const mapped = lineRange(diag.line_start, diag.line_end);
    const range = new vscode.Range(mapped.startLine, 0, mapped.endLine, 1000);
    const item = new vscode.Diagnostic(
      range,
      `texo: stale claim. ${diag.message}`,
      vscode.DiagnosticSeverity.Warning,
    );
    item.source = "texo";
    item.code = diag.claim_id;
    const list = byFile.get(diag.file) ?? [];
    list.push(item);
    byFile.set(diag.file, list);
  }

  collection.clear();
  for (const [file, diagnostics] of byFile) {
    collection.set(vscode.Uri.file(file), diagnostics);
  }
}
