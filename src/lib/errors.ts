/**
 * Messages from Rust and servers are shown as UI text in sentence case: the first letter is
 * capitalised at display time ("permission denied" → "Permission denied"), nothing else changes.
 */
import { toIpcError } from './ipc/client';

export function sentence(text: string): string {
  const trimmed = text.trimStart();
  if (!trimmed) return text;
  const first = Array.from(trimmed)[0]!;
  return first.toLocaleUpperCase() + trimmed.slice(first.length);
}

/** The message of any failure, ready to show. */
export function errorMessage(cause: unknown): string {
  return sentence(toIpcError(cause).message);
}
