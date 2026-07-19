export interface TextRange {
  start: number;
  end: number;
}

/** Keep whitespace at both edges outside the editable range. Model output is
 * full text for the words it saw; it must never consume separators belonging
 * to adjacent text. */
export function protectBoundaryWhitespace(
  value: string,
  start: number,
  end: number,
): TextRange {
  let protectedStart = Math.max(0, Math.min(start, value.length));
  let protectedEnd = Math.max(protectedStart, Math.min(end, value.length));
  while (protectedStart < protectedEnd && /\s/.test(value[protectedStart])) {
    protectedStart += 1;
  }
  while (protectedEnd > protectedStart && /\s/.test(value[protectedEnd - 1])) {
    protectedEnd -= 1;
  }
  return { start: protectedStart, end: protectedEnd };
}

export function replacePreservingBoundaryWhitespace(
  value: string,
  start: number,
  end: number,
  replacement: string,
): { value: string; range: TextRange } {
  const range = protectBoundaryWhitespace(value, start, end);
  return {
    value: value.slice(0, range.start) + replacement + value.slice(range.end),
    range,
  };
}
