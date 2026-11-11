/**
 * The kit's 5px status square, by connection state: filled (connected), a spinner
 * (connecting), hollow (disconnected), danger (failed). Never colour-only: it has a name.
 */
import { Spinner } from '@mnml-tools/ui';
import type { ConnectionStatus } from '@/state/workspace';
import { workspaceStrings as t } from '@/strings/workspace';

export function SessionDot({ status }: { status: ConnectionStatus | 'idle' }) {
  if (status === 'connecting') {
    return (
      <span className="gem-session-spinner" role="img" aria-label={t.status.connecting}>
        <Spinner size={11} />
      </span>
    );
  }
  return (
    <span
      className="mnml-status-dot gem-session-dot"
      data-state={status}
      role="img"
      aria-label={t.status[status]}
    />
  );
}
