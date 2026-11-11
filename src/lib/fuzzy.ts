/**
 * Fuzzy matching for the command palette, after Zap's Go To: exact, prefix, word-start
 * substring, substring, then the query's characters in order. Lower rank is better.
 */
export type FuzzyMatch = { rank: number; indices: number[] };

const range = (from: number, length: number) => Array.from({ length }, (_, i) => from + i);
const wordStart = (text: string, index: number) =>
  index === 0 || /[\s\-_./:|]/.test(text[index - 1]!);

export function fuzzyMatch(text: string, search: string): FuzzyMatch | null {
  const query = search.trim().toLowerCase();
  const lower = text.toLowerCase();
  if (!query) return { rank: 0, indices: [] };
  if (lower === query) return { rank: 0, indices: range(0, query.length) };
  if (lower.startsWith(query)) return { rank: 1, indices: range(0, query.length) };
  let first = -1;
  for (let index = lower.indexOf(query); index >= 0; index = lower.indexOf(query, index + 1)) {
    if (wordStart(text, index)) return { rank: 2, indices: range(index, query.length) };
    if (first < 0) first = index;
  }
  if (first >= 0) return { rank: 3, indices: range(first, query.length) };
  const indices: number[] = [];
  let position = 0;
  for (const char of query) {
    if (char === ' ') continue;
    const found = lower.indexOf(char, position);
    if (found < 0) return null;
    indices.push(found);
    position = found + 1;
  }
  return { rank: 4, indices };
}
