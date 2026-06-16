import * as path from "node:path";
import * as vscode from "vscode";
import { lineRange } from "./range";
import { StalenessReport } from "./runner";

/**
 * Create the diagnostic collection. The caller is responsible for disposing it
 * (e.g. by pushing it onto `context.subscriptions`).
 */
export function createDiagnosticCollection(): vscode.DiagnosticCollection {
  return vscode.languages.createDiagnosticCollection("texo");
}

function resolveAbsolute(file: string, root: string): string {
  return path.isAbsolute(file) ? file : path.join(root, file);
}

/**
 * Publish diagnostics from a staleness report into `collection`.
 *
 * Only files within `scope` are affected: each scoped file is set to the
 * diagnostics reported for it (or cleared when none are reported). Diagnostics
 * for files outside `scope` are left untouched, so a single-file or on-save
 * check never wipes results for other files.
 *
 * `scope` holds the absolute paths that were checked. When omitted, every file
 * present in the report is treated as scoped (a full-report publish).
 */
export function publishDiagnostics(
  collection: vscode.DiagnosticCollection,
  report: StalenessReport,
  root: string,
  scope?: Iterable<string>,
): void {
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
    const absolute = resolveAbsolute(diag.file, root);
    const list = byFile.get(absolute) ?? [];
    list.push(item);
    byFile.set(absolute, list);
  }

  const scopedFiles = new Set<string>();
  if (scope === undefined) {
    for (const file of byFile.keys()) {
      scopedFiles.add(file);
    }
  } else {
    for (const file of scope) {
      scopedFiles.add(resolveAbsolute(file, root));
    }
  }

  // Set diagnostics for reported files that fall within scope.
  for (const [absolute, diagnostics] of byFile) {
    if (scopedFiles.has(absolute)) {
      collection.set(vscode.Uri.file(absolute), diagnostics);
    }
  }

  // Clear scoped files that had no diagnostics this run (now clean).
  for (const absolute of scopedFiles) {
    if (!byFile.has(absolute)) {
      collection.delete(vscode.Uri.file(absolute));
    }
  }
}
