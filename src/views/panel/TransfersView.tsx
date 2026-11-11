/**
 * The transfer queue of a connection tab (plan §2.6, §3.5): one 27px row per job with its
 * direction, name, remote path, progress, speed and state; Cancel / Retry per row; conflict
 * rows offer Overwrite, Skip or Keep both ("Apply to all" in the toolbar).
 */
import { Button, EmptyState } from '@mnml-tools/ui';
import { ArrowDownToLine, ArrowUpFromLine, ArrowLeftRight } from 'lucide-react';
import type { CSSProperties } from 'react';
import { ProgressBar } from '@/components/ui/ProgressBar/ProgressBar';
import { useToast } from '@/components/ui/Toast/context';
import { VirtualList } from '@/components/ui/VirtualList/VirtualList';
import { formatSize, formatSpeed } from '@/lib/format';
import { api } from '@/lib/ipc/api';
import type { Transfer } from '@/lib/ipc/bindings/Transfer';
import { errorMessage } from '@/lib/errors';
import { useSessionData } from '@/state/sessionData';
import { panelStrings } from '@/strings/panel';

const t = panelStrings.transfers_;
const ROW_HEIGHT = 27;

type Props = { transfers: Transfer[]; applyToAll: boolean };

function tone(transfer: Transfer) {
  if (transfer.state === 'failed') return 'danger' as const;
  if (transfer.state === 'done') return 'done' as const;
  if (transfer.state === 'cancelled' || transfer.state === 'skipped') return 'muted' as const;
  return 'default' as const;
}

export function TransfersView({ transfers, applyToAll }: Props) {
  const speeds = useSessionData((state) => state.speeds);
  const toast = useToast();

  const run = (action: Promise<unknown>) =>
    action.catch((cause: unknown) => toast.show({ message: errorMessage(cause), tone: 'error' }));

  const renderRow = (index: number, style: CSSProperties) => {
    const transfer = transfers[index]!;
    const value =
      transfer.state === 'done'
        ? 1
        : transfer.size
          ? transfer.done / transfer.size
          : transfer.state === 'running'
            ? null
            : 0;
    const speed = speeds[transfer.id];
    return (
      <div
        key={transfer.id}
        role="row"
        className="gem-transfer-row"
        data-state={transfer.state}
        style={style}
      >
        <span role="cell" className="gem-transfer-direction">
          {transfer.direction === 'upload' ? (
            <ArrowUpFromLine size={13} aria-label={t.upload} />
          ) : (
            <ArrowDownToLine size={13} aria-label={t.download} />
          )}
        </span>
        <span role="cell" className="gem-transfer-what">
          <span className="gem-transfer-name" title={transfer.localPath}>
            {transfer.name}
          </span>
          <span className="gem-transfer-path" title={transfer.remotePath}>
            {transfer.remotePath}
          </span>
        </span>
        <span role="cell" className="gem-transfer-progress">
          <ProgressBar value={value} tone={tone(transfer)} label={t.progress(transfer.name)} />
        </span>
        <span role="cell" className="gem-transfer-size">
          {transfer.state === 'running' && speed
            ? formatSpeed(speed)
            : transfer.size != null
              ? formatSize(transfer.size)
              : ''}
        </span>
        <span role="cell" className="gem-transfer-state" title={transfer.error ?? undefined}>
          {transfer.state === 'failed' && transfer.error
            ? `${t.states.failed}: ${transfer.error}`
            : transfer.state === 'conflict'
              ? t.conflict(transfer.name)
              : transfer.state === 'running' && transfer.size
                ? `${Math.floor((transfer.done / transfer.size) * 100)}%`
                : t.states[transfer.state]}
        </span>
        <span role="cell" className="gem-transfer-actions">
          {transfer.state === 'conflict' && (
            <>
              <Button
                onClick={() => run(api.resolveConflict(transfer.id, 'overwrite', applyToAll))}
              >
                {t.overwrite}
              </Button>
              <Button onClick={() => run(api.resolveConflict(transfer.id, 'skip', applyToAll))}>
                {t.skip}
              </Button>
              <Button onClick={() => run(api.resolveConflict(transfer.id, 'rename', applyToAll))}>
                {t.rename}
              </Button>
            </>
          )}
          {(transfer.state === 'queued' || transfer.state === 'running') && (
            <Button onClick={() => run(api.cancelTransfer(transfer.id))}>{t.cancel}</Button>
          )}
          {(transfer.state === 'failed' || transfer.state === 'cancelled') && (
            <Button onClick={() => run(api.retryTransfer(transfer.id))}>{t.retry}</Button>
          )}
        </span>
      </div>
    );
  };

  if (!transfers.length) {
    return (
      <EmptyState
        icon={<ArrowLeftRight size={20} aria-hidden />}
        title={t.empty}
        className="gem-panel-empty"
      >
        {t.emptyHint}
      </EmptyState>
    );
  }

  return (
    <VirtualList
      count={transfers.length}
      rowHeight={ROW_HEIGHT}
      renderRow={renderRow}
      className="gem-transfer-list"
      data-conflicts={transfers.some((item) => item.state === 'conflict') || undefined}
      role="table"
      aria-label={t.label}
      aria-rowcount={transfers.length}
    />
  );
}
