/**
 * The bottom panel of the active connection tab (plan §3.5): a 32px strip with the Log ·
 * Transfers switch and the view's actions, then the view. Collapsed (Mod+J), only the strip
 * stays, so the Log toggle can still show its error marker; the panel never opens by itself.
 */
import { IconButton, Segmented } from '@mnml-tools/ui';
import { ChevronDown, ChevronUp, Copy, Eraser, FolderOpen, ListChecks, Save } from 'lucide-react';
import { useEffect, useState } from 'react';
import { useToast } from '@/components/ui/Toast/context';
import { formatClock } from '@/lib/format';
import { api } from '@/lib/ipc/api';
import { errorMessage } from '@/lib/errors';
import { keys, SHORTCUTS } from '@/lib/shortcuts';
import { FINISHED_STATES, logBuffer, useSessionData, useTransfers } from '@/state/sessionData';
import type { ConnectionTab } from '@/state/workspace';
import { panelStrings as t } from '@/strings/panel';
import { LogView, type LogSelection } from './LogView';
import { TransfersView } from './TransfersView';

const EMPTY_SELECTION: LogSelection = new Set();

export function BottomPanel({ tab }: { tab: ConnectionTab }) {
  const connId = tab.id;
  const open = useSessionData((state) => state.panelOpen);
  const view = useSessionData((state) => state.panelView);
  const unseenError = useSessionData((state) => !!state.unseenError[connId]);
  const transfers = useTransfers(connId);
  const toast = useToast();
  // Line selection belongs to the tab whose log it indexes.
  const [selected, setSelected] = useState<{ connId: string; lines: LogSelection }>({
    connId,
    lines: new Set(),
  });
  const selection = selected.connId === connId ? selected.lines : EMPTY_SELECTION;
  const setSelection = (lines: LogSelection) => setSelected({ connId, lines });
  const [applyToAll, setApplyToAll] = useState(false);
  const data = useSessionData.getState();

  const active = transfers.filter((item) => !FINISHED_STATES.has(item.state)).length;
  const conflicts = transfers.some((item) => item.state === 'conflict');
  const logShown = open && view === 'log';

  useEffect(() => {
    if (logShown) useSessionData.getState().markLogSeen(connId);
  }, [logShown, connId, unseenError]);

  const show = (next: 'log' | 'transfers') => {
    data.setPanelView(next);
    if (!open) data.setPanelOpen(true);
  };

  const logText = (indices?: LogSelection) => {
    const buffer = logBuffer(connId);
    const lines = indices?.size
      ? [...indices].sort((a, b) => a - b).map((index) => buffer.at(index))
      : buffer.toArray();
    return lines
      .filter((line) => line != null)
      .map((line) => `${formatClock(line.ts)}  ${line.kind.padEnd(8)}  ${line.text}`)
      .join('\n');
  };

  const copyLog = async () => {
    const text = logText(selection);
    try {
      await navigator.clipboard.writeText(text);
      toast.show({ message: t.log_.copied(text ? text.split('\n').length : 0) });
    } catch {
      toast.show({ message: t.log_.copy, tone: 'error' });
    }
  };

  const saveLog = async () => {
    try {
      const stamp = new Date().toISOString().slice(0, 19).replace(/[:T]/g, '-');
      const name = `gem-${tab.label.replace(/[^\w.-]+/g, '_')}-${stamp}.log`;
      if (await api.saveLog(logText(), name)) toast.show({ message: t.log_.saved });
    } catch (cause) {
      toast.show({ message: errorMessage(cause), tone: 'error' });
    }
  };

  const clearFinished = async () => {
    try {
      if (tab.session) await api.clearTransfers(tab.session.id);
      data.removeFinished(connId);
    } catch (cause) {
      toast.show({ message: errorMessage(cause), tone: 'error' });
    }
  };

  return (
    <section className="gem-panel" aria-label={t.label} data-open={open || undefined}>
      <div className="gem-panel-bar">
        <Segmented label={t.views} role="tablist" className="gem-panel-switch">
          <button
            type="button"
            role="tab"
            aria-selected={open && view === 'log'}
            onClick={() => (open && view === 'log' ? data.setPanelOpen(false) : show('log'))}
          >
            {t.log}
            {unseenError && !logShown && (
              <span className="gem-error-marker" role="img" aria-label={t.errorMarker} />
            )}
          </button>
          <button
            type="button"
            role="tab"
            aria-selected={open && view === 'transfers'}
            onClick={() =>
              open && view === 'transfers' ? data.setPanelOpen(false) : show('transfers')
            }
          >
            {t.transfers}
            {transfers.length > 0 && (
              <span className="gem-count">{active || transfers.length}</span>
            )}
          </button>
        </Segmented>
        <div className="gem-panel-actions">
          {open && view === 'log' && (
            <>
              <IconButton
                aria-label={selection.size ? t.log_.copySelected : t.log_.copyAll}
                title={selection.size ? t.log_.copySelected : t.log_.copyAll}
                onClick={() => void copyLog()}
              >
                <Copy size={13} aria-hidden />
              </IconButton>
              <IconButton
                aria-label={t.log_.save}
                title={t.log_.save}
                onClick={() => void saveLog()}
              >
                <Save size={13} aria-hidden />
              </IconButton>
              <IconButton
                aria-label={t.log_.clear}
                title={t.log_.clear}
                onClick={() => {
                  setSelection(new Set());
                  data.clearLog(connId);
                }}
              >
                <Eraser size={13} aria-hidden />
              </IconButton>
            </>
          )}
          {open && view === 'transfers' && (
            <>
              {conflicts && (
                <label className="gem-check gem-apply-all">
                  <input
                    type="checkbox"
                    checked={applyToAll}
                    onChange={(event) => setApplyToAll(event.target.checked)}
                  />
                  <span>{t.transfers_.applyToAll}</span>
                </label>
              )}
              <IconButton
                aria-label={t.transfers_.clearFinished}
                title={t.transfers_.clearFinished}
                disabled={!transfers.some((item) => FINISHED_STATES.has(item.state))}
                onClick={() => void clearFinished()}
              >
                <ListChecks size={13} aria-hidden />
              </IconButton>
              <IconButton
                aria-label={t.transfers_.openFolder}
                title={t.transfers_.openFolder}
                onClick={() =>
                  void api
                    .openDownloadFolder()
                    .catch((cause: unknown) =>
                      toast.show({ message: errorMessage(cause), tone: 'error' }),
                    )
                }
              >
                <FolderOpen size={13} aria-hidden />
              </IconButton>
            </>
          )}
          <IconButton
            aria-label={open ? t.hide : t.show}
            title={`${open ? t.hide : t.show} (${keys(SHORTCUTS.togglePanel)})`}
            aria-expanded={open}
            onClick={() => data.togglePanel()}
          >
            {open ? <ChevronDown size={14} aria-hidden /> : <ChevronUp size={14} aria-hidden />}
          </IconButton>
        </div>
      </div>
      {open && (
        <div
          className="gem-panel-body"
          role="tabpanel"
          aria-label={view === 'log' ? t.log : t.transfers}
        >
          {view === 'log' ? (
            <LogView connId={connId} selection={selection} onSelectionChange={setSelection} />
          ) : (
            <TransfersView transfers={transfers} applyToAll={applyToAll} />
          )}
        </div>
      )}
    </section>
  );
}
