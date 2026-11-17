/**
 * Fuzzy subsequence scoring, shared by the command palette and the
 * repositories overview search (单一实现，勿复制).
 *
 * Scoring: exact substring beats subsequence; earlier + tighter matches
 * score higher; consecutive-character runs (acronym-like hits) get a bonus;
 * each gap costs. Returns `null` when the target doesn't match at all.
 */
export function fuzzyScore(query: string, target: string): number | null {
  const q = query.toLowerCase().trim();
  const s = target.toLowerCase();
  if (!q) return 0;
  const idx = s.indexOf(q);
  if (idx >= 0) return 1000 - idx * 5 - (s.length - q.length) + (idx === 0 ? 200 : 0);
  // Subsequence match (fuzzy).
  let ti = 0;
  let last = -2;
  let sc = 0;
  let gaps = 0;
  for (const ch of q) {
    const found = s.indexOf(ch, ti);
    if (found === -1) return null;
    if (found === last + 1) sc += 15;
    else {
      sc += 5;
      gaps++;
    }
    last = found;
    ti = found + 1;
  }
  return sc - gaps * 10;
}
