/**
 * Secrets typed for one connection (a quick-connect URL password, a prompt answer) wait here,
 * in memory only, until the connection attempt takes them. Nothing here is persisted.
 */
const pending = new Map<string, { secret: string; remember: boolean }>();

export function holdSecret(tabId: string, secret: string, remember = false): void {
  pending.set(tabId, { secret, remember });
}

export function takeSecret(tabId: string): { secret: string; remember: boolean } | null {
  const value = pending.get(tabId) ?? null;
  pending.delete(tabId);
  return value;
}
