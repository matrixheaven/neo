export interface CompletionRange {
  query: string;
  start: number;
  end: number;
}

export function activeCompletionRange(text: string, caret: number): CompletionRange | null {
  const safeCaret = Math.max(0, Math.min(caret, text.length));
  let start = safeCaret;
  while (start > 0 && !/\s/.test(text[start - 1] ?? "")) start -= 1;
  const query = text.slice(start, safeCaret);
  if (!query.startsWith("/") && !query.startsWith("@")) return null;
  if (query.startsWith("@[") || query.includes("]")) return null;
  return { query, start, end: safeCaret };
}

export function replaceCompletion(
  text: string,
  range: CompletionRange,
  value: string,
): { text: string; caret: number } {
  const nextText = `${text.slice(0, range.start)}${value}${text.slice(range.end)}`;
  const caret = range.start + value.length;
  return { text: nextText, caret };
}
