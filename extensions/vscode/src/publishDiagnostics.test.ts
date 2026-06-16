import * as assert from "node:assert/strict";
import { Module } from "node:module";

// --- Minimal `vscode` stub injected before loading modules that import it. ---

class StubUri {
  private constructor(public readonly fsPath: string) {}
  static file(p: string): StubUri {
    return new StubUri(p);
  }
  toString(): string {
    return `file://${this.fsPath}`;
  }
}

class StubRange {
  constructor(
    public readonly startLine: number,
    public readonly startChar: number,
    public readonly endLine: number,
    public readonly endChar: number,
  ) {}
}

class StubDiagnostic {
  public source?: string;
  public code?: string;
  constructor(
    public readonly range: StubRange,
    public readonly message: string,
    public readonly severity: number,
  ) {}
}

class StubDiagnosticCollection {
  public readonly store = new Map<string, StubDiagnostic[]>();
  set(uri: StubUri, diagnostics: StubDiagnostic[]): void {
    this.store.set(uri.toString(), diagnostics);
  }
  delete(uri: StubUri): void {
    this.store.delete(uri.toString());
  }
  clear(): void {
    this.store.clear();
  }
  dispose(): void {
    this.store.clear();
  }
  get(uri: StubUri): StubDiagnostic[] | undefined {
    return this.store.get(uri.toString());
  }
}

const vscodeStub = {
  Uri: StubUri,
  Range: StubRange,
  Diagnostic: StubDiagnostic,
  DiagnosticSeverity: { Warning: 1, Error: 0 },
  languages: {
    createDiagnosticCollection: (_name: string): StubDiagnosticCollection =>
      new StubDiagnosticCollection(),
  },
};

interface LoaderModule {
  _load(request: string, parent: unknown, isMain: boolean): unknown;
}
const loader = Module as unknown as LoaderModule;
const originalLoad = loader._load.bind(loader);
loader._load = (request: string, parent: unknown, isMain: boolean): unknown => {
  if (request === "vscode") {
    return vscodeStub;
  }
  return originalLoad(request, parent, isMain);
};

import { createDiagnosticCollection, publishDiagnostics } from "./diagnostics";
import { parseStalenessReport, isStalenessReport } from "./runner";
import type { StalenessReport } from "./runner";

const root = "/repo";
const fileA = "docs/a.md";
const fileB = "docs/b.md";

function diag(file: string, claimId: string): StalenessReport["diagnostics"][number] {
  return {
    file,
    line_start: 5,
    line_end: 5,
    message: `stale: ${claimId}`,
    claim_id: claimId,
  };
}

// 1. A full workspace publish sets diagnostics for both files.
{
  const collection = createDiagnosticCollection() as unknown as StubDiagnosticCollection;
  const report: StalenessReport = { diagnostics: [diag(fileA, "a1"), diag(fileB, "b1")] };
  publishDiagnostics(
    collection as unknown as import("vscode").DiagnosticCollection,
    report,
    root,
  );
  assert.equal(collection.get(StubUri.file("/repo/docs/a.md"))?.length, 1);
  assert.equal(collection.get(StubUri.file("/repo/docs/b.md"))?.length, 1);
}

// 2. A scoped single-file check on A must NOT wipe B's diagnostics.
{
  const collection = createDiagnosticCollection() as unknown as StubDiagnosticCollection;
  // Seed B with an existing diagnostic.
  publishDiagnostics(
    collection as unknown as import("vscode").DiagnosticCollection,
    { diagnostics: [diag(fileB, "b1")] },
    root,
  );
  assert.equal(collection.get(StubUri.file("/repo/docs/b.md"))?.length, 1);

  // Now run a scoped check on A only; report contains A.
  publishDiagnostics(
    collection as unknown as import("vscode").DiagnosticCollection,
    { diagnostics: [diag(fileA, "a1")] },
    root,
    [`${root}/${fileA}`],
  );

  assert.equal(collection.get(StubUri.file("/repo/docs/a.md"))?.length, 1, "A should be set");
  assert.equal(
    collection.get(StubUri.file("/repo/docs/b.md"))?.length,
    1,
    "B must survive a scoped check on A",
  );
}

// 3. A scoped check on a now-clean file clears only that file.
{
  const collection = createDiagnosticCollection() as unknown as StubDiagnosticCollection;
  publishDiagnostics(
    collection as unknown as import("vscode").DiagnosticCollection,
    { diagnostics: [diag(fileA, "a1"), diag(fileB, "b1")] },
    root,
  );
  // A is rechecked and is now clean (no diagnostics in report).
  publishDiagnostics(
    collection as unknown as import("vscode").DiagnosticCollection,
    { diagnostics: [] },
    root,
    [`${root}/${fileA}`],
  );
  assert.equal(collection.get(StubUri.file("/repo/docs/a.md")), undefined, "A cleared when clean");
  assert.equal(collection.get(StubUri.file("/repo/docs/b.md"))?.length, 1, "B untouched");
}

// 4. parseStalenessReport accepts well-formed CLI output.
{
  const out = JSON.stringify({ diagnostics: [diag(fileA, "a1")] });
  const parsed = parseStalenessReport(out);
  assert.equal(parsed.diagnostics.length, 1);
  assert.equal(parsed.diagnostics[0].claim_id, "a1");
}

// 5. Malformed JSON fails gracefully with a descriptive error (no crash).
{
  assert.throws(() => parseStalenessReport("not json at all"), /failed to parse CLI output/);
}

// 6. Valid JSON of the wrong shape is rejected by the type guard.
{
  assert.equal(isStalenessReport({ diagnostics: [{ file: 1 }] }), false);
  assert.equal(isStalenessReport({ foo: "bar" }), false);
  assert.equal(isStalenessReport(null), false);
  assert.throws(
    () => parseStalenessReport(JSON.stringify({ diagnostics: [{ file: 1 }] })),
    /did not match the expected/,
  );
}

loader._load = originalLoad;
console.log("publishDiagnostics scoped-clear ok");
console.log("runner parse/type-guard ok");
