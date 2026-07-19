export interface BurstRange {
  start: number;
  end: number;
}

interface WordRange extends BurstRange {}

function wordsInRange(value: string, start: number, end: number): WordRange[] {
  const safeStart = Math.max(0, Math.min(start, value.length));
  const safeEnd = Math.max(safeStart, Math.min(end, value.length));
  const words: WordRange[] = [];
  for (const match of value.slice(safeStart, safeEnd).matchAll(/\S+/g)) {
    words.push({
      start: safeStart + match.index,
      end: safeStart + match.index + match[0].length,
    });
  }
  return words;
}

/**
 * Splits every completed full window between `start` and `caret`. A window is
 * complete only when its last word is followed by whitespace or another word,
 * so the word currently under the caret remains available for the idle pass.
 *
 * Each returned range ends at the next word's start (or the caret after
 * trailing whitespace). That keeps separators attached to the frozen chunk;
 * the replacement boundary helper protects them before applying a candidate.
 */
export function completedBurstWindows(
  value: string,
  start: number,
  caret: number,
  windowWords: number,
): BurstRange[] {
  if (windowWords < 1) return [];
  const safeStart = Math.max(0, Math.min(start, value.length));
  const safeCaret = Math.max(safeStart, Math.min(caret, value.length));
  const words = wordsInRange(value, safeStart, safeCaret);
  const windows: BurstRange[] = [];
  let chunkStart = safeStart;

  for (let lastIndex = windowWords - 1; lastIndex < words.length; lastIndex += windowWords) {
    const nextWord = words[lastIndex + 1];
    const lastWord = words[lastIndex];
    const hasTrailingBoundary = lastWord.end < safeCaret;
    if (!nextWord && !hasTrailingBoundary) break;

    const chunkEnd = nextWord?.start ?? safeCaret;
    windows.push({ start: chunkStart, end: chunkEnd });
    chunkStart = chunkEnd;
  }
  return windows;
}

/** Starts a replacement/deletion recovery session at the trailing bounded
 * window rather than retaining offsets from text that no longer exists. */
export function trailingBurstStart(
  value: string,
  caret: number,
  windowWords: number,
): number {
  const safeCaret = Math.max(0, Math.min(caret, value.length));
  const words = wordsInRange(value, 0, safeCaret);
  if (words.length === 0) return safeCaret;
  return words[Math.max(0, words.length - Math.max(1, windowWords))].start;
}
