/**
 * The viewer/editor of a connection tab (plan §2.3, §2.4, §3.4): its file-tab strip, the
 * CodeView, the partial-file bar and the footer. Saving checks for conflicts and asks before
 * a non-atomic overwrite; closing a dirty tab asks first.
 */
import {
  Button,
  ConfirmDialog,
  ContextMenu,
  Dialog,
  EmptyState,
  Spinner,
  Tab,
  Tabs,
  useShortcut,
} from '@mnml-tools/ui';
import { useQueryClient } from '@tanstack/react-query';
import { Binary, Download, FileWarning, FileX, Lock, SquareTerminal } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { CodeView, type CodeViewHandle } from '@/components/ui/CodeView/CodeView';
import { useToast } from '@/components/ui/Toast/context';
import { formatSize } from '@/lib/format';
import { api } from '@/lib/ipc/api';
import { sentence } from '@/lib/errors';
import { toIpcError } from '@/lib/ipc/client';
import { parentPath } from '@/lib/paths';
import { useSettings } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { patchListingEntry } from '@/lib/query/listing';
import { SHORTCUTS } from '@/lib/shortcuts';
import { useConnFiles, useFileTabs, type FileTab } from '@/state/fileTabs';
import { registerFileTabCloser } from '@/state/viewerCommands';
import type { ConnectionTab } from '@/state/workspace';
import { common } from '@/strings/common';
import { commandStrings } from '@/strings/commands';
import { viewerStrings as t } from '@/strings/viewer';
import { RegionError } from '../common/RegionError';
import { CommandPane } from '../commands/CommandPane';
import { closeCommandTab } from '../commands/runner';
import { FileIcon } from '../connection/EntryIcon';
import { loadFile } from './fileLoader';
import { useLanguageSupport } from './useLanguageSupport';
import { ViewerFooter } from './ViewerFooter';

const WRAP_KEY = 'gem.wrap';

function readWrap(): boolean {
  try {
    return localStorage.getItem(WRAP_KEY) === 'true';
  } catch {
    return false;
  }
}

type Props = {
  tab: ConnectionTab;
  active: boolean;
  connected: boolean;
  onFocusList: () => void;
  onDownload: (path: string) => void;
};

type ViewerDialog =
  | { kind: 'conflict'; key: string }
  | { kind: 'inPlace'; key: string; force: boolean }
  | { kind: 'unsaved'; key: string }
  | { kind: 'loadAll'; key: string }
  | { kind: 'reloadDirty'; key: string }
  | { kind: 'stopClose'; key: string };

/** Whether a tab shows its text in the CodeView. */
const isText = (file: FileTab | undefined) =>
  file?.status === 'ready' && !!file.meta && !file.meta.binary;

export function ViewerPane({ tab, active, connected, onFocusList, onDownload }: Props) {
  const connId = tab.id;
  const sessionId = tab.session!.id;
  const { tabs, activeKey } = useConnFiles(connId);
  const files = useFileTabs.getState();
  const settings = useSettings();
  const toast = useToast();
  const queryClient = useQueryClient();
  const code = useRef<CodeViewHandle>(null);
  const [dialog, setDialog] = useState<ViewerDialog | null>(null);
  const [cursor, setCursor] = useState({ line: 1, column: 1 });
  const [wrap, setWrap] = useState(readWrap);
  const [menu, setMenu] = useState<{ key: string; x: number; y: number } | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);

  const current = tabs.find((file) => file.key === activeKey);
  /**
   * The CodeView stays mounted (hidden) while a binary, loading or failed tab shows, so the
   * other documents keep their text, undo history and scroll position.
   */
  const codeFile = isText(current) ? current : tabs.find(isText);
  const language = useLanguageSupport(codeFile?.language ?? null);

  const toggleWrap = useCallback(() => {
    setWrap((value) => {
      try {
        localStorage.setItem(WRAP_KEY, String(!value));
      } catch {
        /* Session only. */
      }
      return !value;
    });
  }, []);

  const readOnlyReason = (file: FileTab | undefined) => file?.meta?.readOnly ?? null;

  // ---- Save ----

  const save = useCallback(
    async (key: string, options: { force?: boolean; allowInPlace?: boolean } = {}) => {
      const file = useFileTabs.getState().byConn[connId]?.tabs.find((item) => item.key === key);
      if (!file?.meta || file.status !== 'ready' || file.meta.readOnly || file.busy) return false;
      if (!connected) {
        toast.show({ message: t.disconnectedSave, tone: 'error' });
        return false;
      }
      const text = code.current?.getText(key);
      if (text == null) return false;
      files.update(connId, key, { busy: true });
      try {
        const saved = await api.saveFile(sessionId, file.path, {
          text,
          encoding: file.meta.encoding,
          bom: file.meta.bom,
          lineEnding: file.meta.lineEnding,
          expectedSize: file.meta.size,
          expectedMtime: file.meta.mtime,
          force: options.force ?? false,
          allowInPlace: options.allowInPlace ?? false,
        });
        const latest = useFileTabs.getState().byConn[connId]?.tabs.find((item) => item.key === key);
        if (latest?.meta) {
          files.update(connId, key, {
            busy: false,
            meta: {
              ...latest.meta,
              size: saved.size,
              mtime: saved.mtime,
              nextOffset: saved.size,
              mixedLineEndings: false,
            },
            knownSize: saved.size,
          });
        }
        patchListingEntry(queryClient, sessionId, file.path, {
          size: saved.size,
          mtime: saved.mtime,
        });
        // Only what was sent is saved: typing during the save keeps the tab dirty.
        if (code.current?.getText(key) === text) code.current.markSaved(key);
        toast.show({ message: saved.atomic ? t.saved : t.savedNotAtomic });
        void queryClient.invalidateQueries({
          queryKey: queryKeys.listing(sessionId, parentPath(file.path)),
        });
        return true;
      } catch (cause) {
        files.update(connId, key, { busy: false });
        const error = toIpcError(cause);
        if (error.code === 'conflict') setDialog({ kind: 'conflict', key });
        else if (error.code === 'unsupported' && !options.allowInPlace) {
          setDialog({ kind: 'inPlace', key, force: options.force ?? false });
        } else toast.show({ message: t.saveFailed(error.message), tone: 'error' });
        return false;
      }
    },
    [connId, connected, files, queryClient, sessionId, toast],
  );

  const reload = useCallback(
    (key: string, whole?: boolean) => {
      const file = useFileTabs.getState().byConn[connId]?.tabs.find((item) => item.key === key);
      if (!file) return;
      void loadFile(connId, sessionId, key, whole ?? file.whole, queryClient).then((result) => {
        if (!result.ok && result.error)
          toast.show({ message: sentence(result.error.message), tone: 'error' });
      });
    },
    [connId, sessionId, toast, queryClient],
  );

  useShortcut(SHORTCUTS.save, active && !!current, () => {
    if (current) void save(current.key);
  });
  useShortcut(SHORTCUTS.wrap, active && !!codeFile, toggleWrap);

  // ---- Closing ----

  const closeNow = (key: string) => {
    if (key.startsWith('cmd:')) return closeCommandTab(connId, key);
    code.current?.forget(key);
    files.close(connId, key);
  };

  const requestClose = (key: string) => {
    const file = tabs.find((item) => item.key === key);
    if (file?.command?.state === 'running') {
      files.select(connId, key);
      setDialog({ kind: 'stopClose', key });
    } else if (file?.dirty) {
      files.select(connId, key);
      setDialog({ kind: 'unsaved', key });
    } else closeNow(key);
  };

  // Mod+W with the focus in the viewer closes the active file tab (the window asks us).
  const requestCloseRef = useRef(requestClose);
  useEffect(() => {
    requestCloseRef.current = requestClose;
  });
  useEffect(
    () =>
      registerFileTabCloser(connId, () => {
        const key = useFileTabs.getState().byConn[connId]?.activeKey;
        if (key) requestCloseRef.current(key);
      }),
    [connId],
  );

  // ---- Partial files ----

  const loadMore = async (file: FileTab) => {
    if (!file.meta || loadingMore) return;
    setLoadingMore(true);
    try {
      const chunk = await api.readMore(sessionId, file.meta);
      code.current?.append(file.key, chunk.text);
      const latest = useFileTabs.getState().byConn[connId]?.tabs.find((i) => i.key === file.key);
      if (latest?.meta) {
        files.update(connId, file.key, {
          meta: { ...latest.meta, nextOffset: chunk.nextOffset, complete: chunk.complete },
        });
      }
    } catch (cause) {
      const error = toIpcError(cause);
      toast.show({
        message: sentence(error.message),
        tone: 'error',
        action:
          error.code === 'conflict'
            ? { label: t.tabMenu.reload, run: () => reload(file.key) }
            : undefined,
      });
    } finally {
      setLoadingMore(false);
    }
  };

  const loadAll = (file: FileTab, confirmed = false) => {
    const size = file.meta?.size ?? file.knownSize ?? 0;
    if (!confirmed && size > settings.confirmAboveMib * 1024 * 1024) {
      setDialog({ kind: 'loadAll', key: file.key });
      return;
    }
    reload(file.key, true);
  };

  // ---- Rendering ----

  const body = () => {
    if (!current) return null;
    if (current.status === 'loading') {
      return (
        <div className="gem-viewer-state" aria-busy>
          <Spinner size={16} />
          <span>{t.loading}</span>
        </div>
      );
    }
    if (current.status === 'confirm') {
      const size = formatSize(current.knownSize);
      return (
        <EmptyState icon={<FileWarning size={28} aria-hidden />} title={t.confirmTitle(size)}>
          {t.confirmHint(formatSize(settings.previewWindowKib * 1024))}
          <span className="gem-empty-action">
            <Button
              variant="primary"
              onClick={() => {
                files.update(connId, current.key, { status: 'loading' });
                void loadFile(connId, sessionId, current.key, false, queryClient);
              }}
            >
              {t.confirmOpen}
            </Button>
          </span>
        </EmptyState>
      );
    }
    if (current.status === 'error') {
      return (
        <RegionError
          icon={<FileX size={28} aria-hidden />}
          title={t.errorTitle}
          message={current.error?.message}
        >
          <Button onClick={() => reload(current.key)} disabled={!connected}>
            {common.retry}
          </Button>
        </RegionError>
      );
    }
    if (current.meta?.binary) {
      return (
        <EmptyState icon={<Binary size={28} aria-hidden />} title={t.binaryTitle}>
          {t.binaryHint}
          <span className="gem-empty-action">
            <Button onClick={() => onDownload(current.path)} disabled={!connected}>
              <Download size={13} aria-hidden />
              {t.download}
            </Button>
          </span>
        </EmptyState>
      );
    }
    return null;
  };

  const partial = isText(current) && current!.meta!.readOnly === 'partial' ? current!.meta! : null;
  const reason = readOnlyReason(current);
  const dialogFile = dialog ? tabs.find((file) => file.key === dialog.key) : undefined;

  return (
    <section className="gem-viewer" aria-label={t.tabsLabel}>
      <Tabs className="gem-file-tabs">
        {tabs.map((file) => (
          <Tab
            key={file.key}
            title={
              <span
                className="gem-file-tab-title"
                data-preview={file.preview || undefined}
                data-command={file.command ? true : undefined}
                title={
                  file.command
                    ? (file.command.sent ?? file.command.command)
                    : file.preview
                      ? `${file.path}\n${t.previewHint}`
                      : file.path
                }
                // Double click keeps a preview tab open (VS Code); "Keep open" in the tab menu and
                // editing do too, so the pointer-only gesture has keyboard equivalents.
                onDoubleClick={() => files.pin(connId, file.key)}
              >
                {file.name}
              </span>
            }
            icon={
              file.command ? (
                file.command.state === 'running' ? (
                  <Spinner size={13} />
                ) : (
                  <SquareTerminal size={13} aria-hidden />
                )
              ) : file.busy ? (
                <Spinner size={13} />
              ) : file.meta?.readOnly ? (
                <Lock size={12} aria-hidden />
              ) : (
                <FileIcon name={file.name} size={13} />
              )
            }
            active={file.key === activeKey}
            dirty={file.dirty}
            closeLabel={t.closeFile(file.name)}
            onSelect={() => files.select(connId, file.key)}
            onClose={() => requestClose(file.key)}
            onContextMenu={(event) => {
              event.preventDefault();
              setMenu({ key: file.key, x: event.clientX, y: event.clientY });
            }}
          />
        ))}
      </Tabs>
      <div className="gem-viewer-body">
        {codeFile && (
          <div className="gem-viewer-code" hidden={!isText(current)}>
            <CodeView
              ref={code}
              docKey={codeFile.key}
              initialText={codeFile.initialText}
              docVersion={codeFile.docVersion}
              language={language}
              readOnly={!!codeFile.meta?.readOnly || !connected}
              wrap={wrap}
              label={codeFile.name}
              onDirtyChange={(key, dirty) => {
                const file = useFileTabs.getState().byConn[connId]?.tabs.find((i) => i.key === key);
                files.update(
                  connId,
                  key,
                  dirty && file?.preview ? { dirty, preview: false } : { dirty },
                );
              }}
              onCursor={(line, column) => setCursor({ line, column })}
              onEscape={onFocusList}
            />
          </div>
        )}
        {current?.command ? (
          <CommandPane
            // One pane per command tab: its counters belong to that run.
            key={current.key}
            connId={connId}
            tab={current}
            connected={connected}
            wrap={wrap}
            onEscape={onFocusList}
          />
        ) : (
          body()
        )}
      </div>
      {partial && current && (
        <div className="gem-partial-bar" role="status">
          <span>
            {partial.complete
              ? t.partialAll(formatSize(partial.size))
              : t.partial(formatSize(partial.nextOffset), formatSize(partial.size))}
          </span>
          {!partial.complete && (
            <Button disabled={loadingMore || !connected} onClick={() => void loadMore(current)}>
              {loadingMore && <Spinner size={12} />}
              {t.loadMore}
            </Button>
          )}
          <Button disabled={current.busy || !connected} onClick={() => loadAll(current)}>
            {t.loadAll}
          </Button>
        </div>
      )}
      {current && !current.command && (
        <ViewerFooter
          file={current}
          cursor={isText(current) ? cursor : null}
          readOnly={reason ?? (!connected && isText(current) ? 'disconnected' : null)}
          wrap={wrap}
          onToggleWrap={toggleWrap}
          onLanguage={(language) =>
            files.update(connId, current.key, { language, languageManual: true })
          }
        />
      )}

      {menu && (
        <ContextMenu
          label={t.tabMenu.label(tabs.find((file) => file.key === menu.key)?.name ?? '')}
          x={menu.x}
          y={menu.y}
          onDismiss={() => setMenu(null)}
          items={[
            ...(menu.key.startsWith('cmd:')
              ? []
              : [
                  {
                    label: t.tabMenu.keepOpen,
                    disabled: !tabs.find((file) => file.key === menu.key)?.preview,
                    onSelect: () => {
                      files.pin(connId, menu.key);
                      setMenu(null);
                    },
                  },
                  {
                    label: t.tabMenu.reload,
                    disabled: !connected,
                    onSelect: () => {
                      setMenu(null);
                      const file = tabs.find((item) => item.key === menu.key);
                      if (file?.dirty) setDialog({ kind: 'reloadDirty', key: menu.key });
                      else reload(menu.key);
                    },
                  },
                  {
                    label: t.tabMenu.copyPath,
                    onSelect: () => {
                      setMenu(null);
                      navigator.clipboard
                        .writeText(menu.key)
                        .then(() => toast.show({ message: common.copiedPath }))
                        .catch(() => toast.show({ message: common.copyFailed, tone: 'error' }));
                    },
                  },
                ]),
            {
              label: t.tabMenu.close,
              onSelect: () => {
                setMenu(null);
                requestClose(menu.key);
              },
            },
            {
              label: t.tabMenu.closeOthers,
              disabled: tabs.length < 2,
              onSelect: () => {
                setMenu(null);
                for (const file of tabs)
                  if (file.key !== menu.key && !file.dirty) closeNow(file.key);
              },
            },
          ]}
        />
      )}

      {dialog?.kind === 'conflict' && dialogFile && (
        <Dialog
          title={t.conflict.title}
          role="alertdialog"
          onDismiss={() => setDialog(null)}
          className="mnml-message-dialog gem-conflict-dialog"
        >
          <p>{t.conflict.message(dialogFile.name)}</p>
          <div className="mnml-dialog-actions">
            <Button data-autofocus onClick={() => setDialog(null)}>
              {common.cancel}
            </Button>
            <Button
              onClick={() => {
                setDialog(null);
                reload(dialogFile.key);
              }}
            >
              {t.conflict.reload}
            </Button>
            <Button
              className="gem-button-danger"
              onClick={() => {
                setDialog(null);
                void save(dialogFile.key, { force: true });
              }}
            >
              {t.conflict.overwrite}
            </Button>
          </div>
        </Dialog>
      )}
      {dialog?.kind === 'inPlace' && dialogFile && (
        <ConfirmDialog
          title={t.inPlace.title}
          message={t.inPlace.message}
          confirmLabel={t.inPlace.confirm}
          cancelLabel={common.cancel}
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            const force = dialog.force;
            setDialog(null);
            void save(dialogFile.key, { force, allowInPlace: true });
          }}
        />
      )}
      {dialog?.kind === 'unsaved' && dialogFile && (
        <Dialog
          title={t.unsaved.title}
          role="alertdialog"
          onDismiss={() => setDialog(null)}
          className="mnml-message-dialog"
        >
          <p>{t.unsaved.message(dialogFile.name)}</p>
          <div className="mnml-dialog-actions">
            <Button data-autofocus onClick={() => setDialog(null)}>
              {common.cancel}
            </Button>
            <Button
              className="gem-button-danger"
              onClick={() => {
                setDialog(null);
                closeNow(dialogFile.key);
              }}
            >
              {t.unsaved.discard}
            </Button>
            <Button
              variant="primary"
              disabled={!connected || !!dialogFile.meta?.readOnly}
              onClick={async () => {
                setDialog(null);
                if (await save(dialogFile.key)) closeNow(dialogFile.key);
              }}
            >
              {t.unsaved.save}
            </Button>
          </div>
        </Dialog>
      )}
      {dialog?.kind === 'stopClose' && dialogFile && (
        <ConfirmDialog
          title={commandStrings.closeRunning.title}
          message={commandStrings.closeRunning.message(dialogFile.name)}
          confirmLabel={commandStrings.closeRunning.confirm}
          cancelLabel={common.cancel}
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            setDialog(null);
            closeCommandTab(connId, dialogFile.key);
          }}
        />
      )}
      {dialog?.kind === 'reloadDirty' && dialogFile && (
        <ConfirmDialog
          title={t.reloadDirty.title}
          message={t.reloadDirty.message(dialogFile.name)}
          confirmLabel={t.conflict.reload}
          cancelLabel={common.cancel}
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            setDialog(null);
            reload(dialogFile.key);
          }}
        />
      )}
      {dialog?.kind === 'loadAll' && dialogFile && (
        <ConfirmDialog
          title={t.loadAllTitle}
          message={t.loadAllMessage(formatSize(dialogFile.meta?.size ?? dialogFile.knownSize))}
          confirmLabel={t.loadAllConfirm}
          cancelLabel={common.cancel}
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            setDialog(null);
            loadAll(dialogFile, true);
          }}
        />
      )}
    </section>
  );
}
