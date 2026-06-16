import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as vscode from "vscode";

const execFileAsync = promisify(execFile);

export interface StalenessDiagnostic {
  file: string;
  line_start: number;
  line_end: number;
  message: string;
  claim_id: string;
  superseded_by?: string;
}

export interface StalenessReport {
  diagnostics: StalenessDiagnostic[];
}

function isStalenessDiagnostic(value: unknown): value is StalenessDiagnostic {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    typeof record.file === "string" &&
    typeof record.line_start === "number" &&
    typeof record.line_end === "number" &&
    typeof record.message === "string" &&
    typeof record.claim_id === "string" &&
    (record.superseded_by === undefined || typeof record.superseded_by === "string")
  );
}

/** Runtime guard for untrusted CLI output parsed from JSON. */
export function isStalenessReport(value: unknown): value is StalenessReport {
  if (typeof value !== "object" || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return Array.isArray(record.diagnostics) && record.diagnostics.every(isStalenessDiagnostic);
}

/** Parse and validate untrusted CLI stdout, throwing a descriptive error on malformed output. */
export function parseStalenessReport(stdout: string): StalenessReport {
  let parsed: unknown;
  try {
    parsed = JSON.parse(stdout);
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err);
    throw new Error(`texo: failed to parse CLI output as JSON. ${message}`);
  }
  if (!isStalenessReport(parsed)) {
    throw new Error("texo: CLI output did not match the expected staleness report shape.");
  }
  return parsed;
}

function texoArgs(extra: string[]): string[] {
  const cfg = vscode.workspace.getConfiguration("texo");
  const workspaceId = cfg.get<string>("workspaceId", "demo");
  return ["--workspace", workspaceId, ...extra];
}

export async function runCheckStaleness(root: string, target: string): Promise<StalenessReport> {
  const cfg = vscode.workspace.getConfiguration("texo");
  const binary = cfg.get<string>("binaryPath", "texo");
  const { stdout } = await execFileAsync(binary, texoArgs(["check-staleness", target, "--json"]), {
    cwd: root,
  });
  return parseStalenessReport(stdout);
}

export async function runAgentContext(root: string, outPath: string): Promise<void> {
  const cfg = vscode.workspace.getConfiguration("texo");
  const binary = cfg.get<string>("binaryPath", "texo");
  await execFileAsync(binary, texoArgs(["agent-context", "--out", outPath]), {
    cwd: root,
  });
}
