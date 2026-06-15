import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as vscode from "vscode";

const execFileAsync = promisify(execFile);

export interface StalenessReport {
  diagnostics: Array<{
    file: string;
    line_start: number;
    line_end: number;
    message: string;
    claim_id: string;
    superseded_by?: string;
  }>;
}

function texoArgs(root: string, extra: string[]): string[] {
  const cfg = vscode.workspace.getConfiguration("texo");
  const workspaceId = cfg.get<string>("workspaceId", "demo");
  return ["--workspace", workspaceId, ...extra];
}

export async function runCheckStaleness(root: string, path: string): Promise<StalenessReport> {
  const cfg = vscode.workspace.getConfiguration("texo");
  const binary = cfg.get<string>("binaryPath", "texo");
  const { stdout } = await execFileAsync(
    binary,
    texoArgs(root, ["check-staleness", path, "--json"]),
    { cwd: root },
  );
  return JSON.parse(stdout) as StalenessReport;
}

export async function runAgentContext(root: string, outPath: string): Promise<void> {
  const cfg = vscode.workspace.getConfiguration("texo");
  const binary = cfg.get<string>("binaryPath", "texo");
  await execFileAsync(binary, texoArgs(root, ["agent-context", "--out", outPath]), {
    cwd: root,
  });
}
