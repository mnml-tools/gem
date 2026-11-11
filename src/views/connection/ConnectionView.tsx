/**
 * One connection tab's body (plan §3.1): the connect flow and its prompts, then the file list
 * on the left (resizable, `gem.listWidth`) and the viewer on the right. With no file open the
 * list takes the full width. A dropped session keeps everything on screen under a
 * "Disconnected · Reconnect" strip; open files keep their text and dirty state.
 */
import { Button, EmptyState, ResizeHandle, Spinner, useResize } from '@mnml-tools/ui';
import { Unplug } from 'lucide-react';
import { useQueryClient } from '@tanstack/react-query';
import { useCallback, useEffect, useId, useRef } from 'react';
import { useToast } from '@/components/ui/Toast/context';
import { api } from '@/lib/ipc/api';
import type { Entry } from '@/lib/ipc/bindings/Entry';
import { errorMessage, sentence } from '@/lib/errors';
import { useSettings } from '@/lib/query/hooks';
import { useConnFiles, useFileTabs } from '@/state/fileTabs';
import { useSessionData } from '@/state/sessionData';
import { useWorkspace, type ConnectionTab } from '@/state/workspace';
import { connectionStrings as t } from '@/strings/connection';
import { loadFile } from '../viewer/fileLoader';
import { ViewerPane } from '../viewer/ViewerPane';
import { RegionError } from '../common/RegionError';
import { HostKeyChangedDialog, SecretDialog, TrustDialog } from './ConnectPrompts';
import { FileBrowser, type FileBrowserHandle } from './FileBrowser';
import { useOsDrop } from './useOsDrop';
import { useSession } from './useSession';

type Props = {
  tab: ConnectionTab;
  active: boolean;
  /** Opens the site dialog of a saved site (Edit site, Forget host key). */
  onEditSite: (siteId: string) => void;
};

export function ConnectionView({ tab, active, onEditSite }: Props) {
  const session = useSession(tab);
  const settings = useSettings();
  const toast = useToast();
  const queryClient = useQueryClient();
  const browser = useRef<FileBrowserHandle>(null);
  const { tabs: fileTabs } = useConnFiles(tab.id);
  const listId = useId();
  const resize = useResize({
    storageKey: 'gem.listWidth',
    initial: 380,
    minimum: 220,
    maximum: 1200,
  });
  const connected = tab.status === 'connected';
  const hasFiles = fileTabs.length > 0;

  const focusList = useCallback(() => browser.current?.focus(), []);

  // Closing the last file gives the focus back to the list.
  const hadFiles = useRef(hasFiles);
  useEffect(() => {
    if (hadFiles.current && !hasFiles && active) focusList();
    hadFiles.current = hasFiles;
  }, [hasFiles, active, focusList]);

  const openFile = useCallback(
    (entry: Entry, preview: boolean) => {
      const sessionId = tab.session?.id;
      if (!sessionId) return;
      const size = entry.size ?? null;
      const needsConfirm = size != null && size > settings.confirmAboveMib * 1024 * 1024;
      const { key, load } = useFileTabs
        .getState()
        .open(tab.id, entry.path, { preview, knownSize: size, needsConfirm });
      if (load) void loadFile(tab.id, sessionId, key, false, queryClient);
    },
    [tab.id, tab.session?.id, settings.confirmAboveMib, queryClient],
  );

  const download = useCallback(
    async (path: string) => {
      if (!tab.session) return;
      try {
        const transfers = await api.downloadPaths(tab.session.id, [path], false);
        useSessionData.getState().upsertTransfers(tab.id, transfers);
        if (transfers.length) toast.show({ message: t.transfersStarted(1, true) });
      } catch (cause) {
        toast.show({ message: errorMessage(cause), tone: 'error' });
      }
    },
    [tab.id, tab.session, toast],
  );

  const dropTarget = useOsDrop({
    enabled: active && connected && !!tab.session,
    sessionId: tab.session?.id ?? null,
    connId: tab.id,
    currentDir: tab.path,
  });

  const prompts = (
    <>
      {session.prompt?.kind === 'secret' && (
        <SecretDialog tab={tab} onSubmit={session.submitSecret} onCancel={session.cancelPrompt} />
      )}
      {session.prompt?.kind === 'trust' && (
        <TrustDialog
          hostKey={session.prompt.key}
          tls={session.prompt.tls}
          onTrust={() => void session.trust()}
          onCancel={session.cancelPrompt}
        />
      )}
      {session.prompt?.kind === 'changed' && (
        <HostKeyChangedDialog
          hostKey={session.prompt.key}
          host={tab.host}
          canOpenSite={!!tab.siteId}
          onOpenSite={() => {
            session.cancelPrompt();
            if (tab.siteId) onEditSite(tab.siteId);
          }}
          onDismiss={session.cancelPrompt}
        />
      )}
    </>
  );

  if (!tab.session) {
    return (
      <div className="gem-connection-state">
        {tab.status === 'connecting' ? (
          <EmptyState icon={<Spinner size={20} />} title={t.connecting(tab.label)} />
        ) : (
          <RegionError
            icon={<Unplug size={28} aria-hidden />}
            title={t.failedTitle}
            message={tab.error?.message}
          >
            <Button variant="primary" onClick={() => useWorkspace.getState().retry(tab.id)}>
              {t.retry}
            </Button>
            {tab.siteId && <Button onClick={() => onEditSite(tab.siteId!)}>{t.editSite}</Button>}
          </RegionError>
        )}
        {prompts}
      </div>
    );
  }

  return (
    <div className="gem-connection">
      {!connected && (
        <div className="gem-disconnected" role="status">
          <Unplug size={13} aria-hidden />
          <span>
            {tab.status === 'connecting' ? t.reconnecting : t.disconnected}
            {tab.error && tab.status !== 'connecting' ? ` · ${sentence(tab.error.message)}` : ''}
          </span>
          {tab.status !== 'connecting' && (
            <Button onClick={session.reconnect}>{t.reconnect}</Button>
          )}
        </div>
      )}
      <div className="gem-connection-body">
        <div
          id={listId}
          className="gem-list-pane"
          data-full={!hasFiles || undefined}
          style={hasFiles ? { width: resize.width } : undefined}
        >
          <FileBrowser
            ref={browser}
            tab={tab}
            active={active}
            connected={connected}
            onOpenFile={openFile}
            dropTarget={dropTarget}
          />
          {hasFiles && <ResizeHandle {...resize} label={t.resizeList} controls={listId} />}
        </div>
        {hasFiles && (
          <ViewerPane
            tab={tab}
            active={active}
            connected={connected}
            onFocusList={focusList}
            onDownload={(path) => void download(path)}
          />
        )}
      </div>
      {prompts}
    </div>
  );
}
