import * as assert from "node:assert/strict";
import { lineRange } from "./range";

const sample = {
  diagnostics: [
    {
      file: "sample_sources/stale_onboarding.md",
      line_start: 5,
      line_end: 5,
      message: "Claim appears stale: superseded by claim_abc",
      claim_id: "claim_old",
      superseded_by: "claim_new",
    },
  ],
};

for (const diag of sample.diagnostics) {
  const range = lineRange(diag.line_start, diag.line_end);
  assert.equal(range.startLine, 4);
  assert.equal(range.endLine, 4);
  assert.match(diag.message, /superseded/);
}

console.log("diagnostics mapping ok");
