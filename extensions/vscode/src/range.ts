/** Map texo JSON line numbers to VS Code zero-based ranges. */
export function lineRange(
  lineStart: number,
  lineEnd: number,
): { startLine: number; endLine: number } {
  return {
    startLine: Math.max(lineStart - 1, 0),
    endLine: Math.max(lineEnd - 1, 0),
  };
}
