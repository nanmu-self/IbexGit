/**
 * Word-level (intra-line) diff for paired remove/add lines (PLAN P4:
 * 词级差异高亮). Pure TypeScript, no dependencies; tokenizes on word
 * boundaries and punctuation, then LCS-aligns the token sequences.
 */

export interface WordSpan {
  text: string;
  /** Part of the change (highlighted in the viewer). */
  changed: boolean;
}

const MAX_TOKENS = 220; // skip LCS for pathologically long lines

function tokenize(text: string): string[] {
  return text.split(/(\s+|[A-Za-z0-9_]+|[^\sA-Za-z0-9_])/g).filter((t) => t.length > 0);
}

/**
 * Align `oldText` against `newText` and return per-side spans marking the
 * changed regions. Returns null when the inputs are too large to be worth
 * highlighting (the viewer falls back to plain +/- line rendering).
 */
export function wordDiff(oldText: string, newText: string): [WordSpan[], WordSpan[]] | null {
  if (oldText === newText) return null;
  const a = tokenize(oldText);
  const b = tokenize(newText);
  if (a.length > MAX_TOKENS || b.length > MAX_TOKENS) return null;

  // LCS table (small: both sides bounded).
  const n = a.length;
  const m = b.length;
  const dp: Uint32Array[] = [];
  for (let i = 0; i <= n; i++) dp.push(new Uint32Array(m + 1));
  for (let i = n - 1; i >= 0; i--) {
    for (let j = m - 1; j >= 0; j--) {
      dp[i][j] = a[i] === b[j] ? dp[i + 1][j + 1] + 1 : Math.max(dp[i + 1][j], dp[i][j + 1]);
    }
  }

  function spans(tokens: string[], other: string[]): WordSpan[] {
    const out: WordSpan[] = [];
    let i = 0;
    let j = 0;
    const push = (text: string, changed: boolean) => {
      const last = out[out.length - 1];
      if (last && last.changed === changed) last.text += text;
      else out.push({ text, changed });
    };
    while (i < tokens.length && j < other.length) {
      if (tokens[i] === other[j]) {
        push(tokens[i], false);
        i++;
        j++;
      } else if (dp[i + 1][j] >= dp[i][j + 1]) {
        push(tokens[i], true);
        i++;
      } else {
        j++; // token only exists on the other side; skip here
      }
    }
    while (i < tokens.length) push(tokens[i++], true);
    return out;
  }

  const oldSpans = spans(a, b);
  const newSpans = spans(b, a);
  return [oldSpans, newSpans];
}
