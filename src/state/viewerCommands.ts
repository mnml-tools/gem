/**
 * Commands a connection tab's viewer offers to the window: Mod+W closes the active file tab
 * (asking first when it is dirty) when the focus is in the viewer.
 */
const closers = new Map<string, () => void>();

export function registerFileTabCloser(connId: string, close: () => void): () => void {
  closers.set(connId, close);
  return () => {
    if (closers.get(connId) === close) closers.delete(connId);
  };
}

export function closeActiveFileTab(connId: string): boolean {
  const close = closers.get(connId);
  if (!close) return false;
  close();
  return true;
}
