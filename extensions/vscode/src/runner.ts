import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as vscode from "vscode";

const execFileAsync = promisify(execFile);

/** Default wall-clock budget for a single CLI invocation, in milliseconds. */
const DEFAULT_TIMEOUT_MS = 30_000;

/** Raised when a CLI invocation exceeds its time budget and the child is killed. */
export class TexoTimeoutError extends Error {
  constructor(binary: string, timeoutMs: number) {
    super(`\`${binary}\` timed out after ${timeoutMs}ms; the check was killed.`);
    this.name = "TexoTimeoutError";
  }
}

/** True when `err` came from an AbortSignal cancelling a superseded run. */
export function isAbortError(err: unknown): boolean {
  if (!(err instanceof Error)) {
    return false;
  }
  return err.name === "AbortError" || (err as NodeJS.ErrnoException).code === "ABORT_ERR";
}

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

/**
 * Run the texo CLI with a wall-clock timeout (child killed on expiry) and an
 * optional AbortSignal so a superseded run can be cancelled outright.
 */
async function execTexo(
  root: string,
  extra: string[],
  signal?: AbortSignal,
): Promise<{ stdout: string }> {
  const cfg = vscode.workspace.getConfiguration("texo");
  const binary = cfg.get<string>("binaryPath", "texo");
  const timeoutMs = cfg.get<number>("checkTimeoutMs", DEFAULT_TIMEOUT_MS);
  try {
    return await execFileAsync(binary, texoArgs(extra), {
      cwd: root,
      timeout: timeoutMs,
      killSignal: "SIGTERM",
      signal,
    });
  } catch (err) {
    // An abort is a deliberate cancellation, not a timeout — rethrow as-is.
    // Checked first in case an aborted child also reports `killed`.
    if (isAbortError(err)) {
      throw err;
    }
    if (err instanceof Error && (err as { killed?: boolean }).killed === true) {
      throw new TexoTimeoutError(binary, timeoutMs);
    }
    throw err;
  }
}

export async function runCheckStaleness(
  root: string,
  target: string,
  signal?: AbortSignal,
): Promise<StalenessReport> {
  const { stdout } = await execTexo(root, ["check-staleness", target, "--json"], signal);
  return parseStalenessReport(stdout);
}

export async function runAgentContext(root: string, outPath: string): Promise<void> {
  await execTexo(root, ["agent-context", "--out", outPath]);
}
