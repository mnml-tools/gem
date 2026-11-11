/**
 * The remote file list of a connection tab (plan §2.5, §3.3): path bar, toolbar, sortable
 * columns and a virtual list of 27px rows. The list is one `listbox` (focus stays on it, the
 * current row is its active descendant), so it is one Tab stop and fully keyboard driven.
 */
import {
  Button,
  ConfirmDialog,
  ContextMenu,
  EmptyState,
  ErrorDialog,
  IconButton,
  Message,
  SearchField,
  Spinner,
  useShortcut,
  type ContextMenuItem,
} from '@mnml-tools/ui';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import {
  ArrowUp,
  ChevronDown,
  ChevronUp,
  Copy,
  Download,
  Eye,
  EyeOff,
  FilePlus,
  FileX,
  FolderOpen,
  FolderPlus,
  KeyRound,
  Lock,
  Pencil,
  RefreshCw,
  Trash2,
  Play,
  Settings2,
  SquareTerminal,
  Upload,
  Zap,
} from 'lucide-react';
import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
} from 'react';
import { PathBar, type PathBarHandle } from '@/components/ui/PathBar/PathBar';
import { useToast } from '@/components/ui/Toast/context';
import { VirtualList, type VirtualListHandle } from '@/components/ui/VirtualList/VirtualList';
import { filterEntries, isDirLike, sortEntries, type SortKey, type SortOrder } from '@/lib/entries';
import { formatDate, formatDateTime, formatMode, formatSize } from '@/lib/format';
import { api } from '@/lib/ipc/api';
import type { DeleteReport } from '@/lib/ipc/bindings/DeleteReport';
import type { Entry } from '@/lib/ipc/bindings/Entry';
import { toIpcError } from '@/lib/ipc/client';
import { errorMessage, sentence } from '@/lib/errors';
import { joinPath, parentPath } from '@/lib/paths';
import { queryKeys } from '@/lib/query/keys';
import { useSettings, useSnippets } from '@/lib/query/hooks';
import { useWindowShortcut } from '@/lib/hooks/useWindowShortcut';
import { keys, SHORTCUTS } from '@/lib/shortcuts';
import { useFileTabs } from '@/state/fileTabs';
import { useSessionData } from '@/state/sessionData';
import { useWorkspace, type ConnectionTab } from '@/state/workspace';
import { common } from '@/strings/common';
import { useUi } from '@/state/ui';
import { commandStrings } from '@/strings/commands';
import { connectionStrings } from '@/strings/connection';
import { launchSnippet } from '../commands/runner';
import { EntryIcon } from './EntryIcon';
import { ChmodDialog, NameDialog } from './FileDialogs';

const t = connectionStrings;
const ROW_HEIGHT = 27;

export type FileBrowserHandle = { focus: () => void };

type Props = {
  tab: ConnectionTab;
  active: boolean;
  /** Writes are refused while the session is down; the listing stays visible. */
  connected: boolean;
  onOpenFile: (entry: Entry, preview: boolean) => void;
  /** Folder under an OS drag (`gem://drag`), or the current folder; `null` when not dragging. */
  dropTarget: string | null;
};

type Dialog =
  | { kind: 'newFolder' }
  | { kind: 'newFile' }
  | { kind: 'rename'; entry: Entry }
  | { kind: 'chmod'; entry: Entry }
  | { kind: 'delete'; entries: Entry[] }
  | { kind: 'report'; report: DeleteReport };

type Menu = {
  x: number;
  y: number;
  label: string;
  items: ContextMenuItem[];
  trigger?: HTMLElement;
};

const COLUMNS: { key: SortKey; label: string }[] = [
  { key: 'name', label: t.columns.name },
  { key: 'size', label: t.columns.size },
  { key: 'modified', label: t.columns.modified },
  { key: 'permissions', label: t.columns.permissions },
  { key: 'owner', label: t.columns.owner },
];

/** True once `flag` has stayed on for `ms` for this `key` (Piw: no spinner for 300ms). */
function useDelayed(flag: boolean, key: string, ms = 300): boolean {
  const [elapsedFor, setElapsedFor] = useState<string | null>(null);
  useEffect(() => {
    if (!flag) return;
    const timer = window.setTimeout(() => setElapsedFor(key), ms);
    return () => window.clearTimeout(timer);
  }, [flag, key, ms]);
  return flag && elapsedFor === key;
}

function rowId(connId: string, path: string): string {
  return `${connId}-row-${encodeURIComponent(path)}`;
}

export const FileBrowser = forwardRef<FileBrowserHandle, Props>(function FileBrowser(
  { tab, active, connected, onOpenFile, dropTarget },
  ref,
) {
  const session = tab.session!;
  const path = tab.path ?? session.startPath;
  const settings = useSettings();
  const queryClient = useQueryClient();
  const toast = useToast();
  const updateConnection = useWorkspace((state) => state.updateConnection);

  const [sort, setSort] = useState<SortOrder>({ key: 'name', descending: false });
  const [filter, setFilter] = useState('');
  const [showHidden, setShowHidden] = useState(settings.showHidden);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [cursor, setCursor] = useState<string | null>(null);
  const anchor = useRef<string | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [dialog, setDialog] = useState<Dialog | null>(null);
  const exec = session.capabilities.exec;
  const snippets = useSnippets(tab.siteId, exec);
  const [snippetMenu, setSnippetMenu] = useState<{
    x: number;
    y: number;
    trigger: HTMLElement;
  } | null>(null);
  const [uploadMenu, setUploadMenu] = useState<{
    x: number;
    y: number;
    trigger: HTMLElement;
  } | null>(null);
  /** After going up, the folder we came from gets the cursor. */
  const pendingCursor = useRef<string | null>(null);

  const list = useRef<VirtualListHandle>(null);
  const filterInput = useRef<HTMLInputElement>(null);
  const pathBar = useRef<PathBarHandle>(null);

  const listing = useQuery({
    queryKey: queryKeys.listing(session.id, path),
    queryFn: () => api.listDir(session.id, path),
    enabled: connected,
  });
  const error = listing.error ? toIpcError(listing.error) : null;
  const showLoading = useDelayed(listing.isFetching && !listing.data, path);

  const visible = useMemo(() => {
    const entries = listing.data?.entries ?? [];
    return sortEntries(filterEntries(entries, showHidden, filter), sort);
  }, [listing.data, showHidden, filter, sort]);
  const indexByPath = useMemo(
    () => new Map(visible.map((entry, index) => [entry.path, index])),
    [visible],
  );
  const hiddenCount = useMemo(
    () => (listing.data?.entries ?? []).filter((entry) => entry.name.startsWith('.')).length,
    [listing.data],
  );
  const names = useMemo(
    () => new Set((listing.data?.entries ?? []).map((entry) => entry.name)),
    [listing.data],
  );

  useEffect(() => {
    updateConnection(tab.id, { items: listing.data ? visible.length : null });
  }, [tab.id, visible.length, listing.data, updateConnection]);

  // The server may answer with the canonical path (symlinks, `..`): adopt it.
  useEffect(() => {
    const canonical = listing.data?.path;
    if (canonical && canonical !== path && listing.data) {
      queryClient.setQueryData(queryKeys.listing(session.id, canonical), listing.data);
      updateConnection(tab.id, { path: canonical });
    }
  }, [listing.data, path, queryClient, session.id, tab.id, updateConnection]);

  // Place the cursor after a folder change (the folder we came from, or the first row).
  useEffect(() => {
    if (!listing.data) return;
    const wanted = pendingCursor.current;
    if (wanted && indexByPath.has(wanted)) {
      pendingCursor.current = null;
      setCursor(wanted);
      setSelected(new Set([wanted]));
      anchor.current = wanted;
      list.current?.scrollToIndex(indexByPath.get(wanted)!);
    } else if (cursor === null || !indexByPath.has(cursor)) {
      setCursor(visible[0]?.path ?? null);
    }
  }, [listing.data, indexByPath, visible, cursor]);

  const focusList = useCallback(() => list.current?.element?.focus(), []);
  useImperativeHandle(ref, () => ({ focus: focusList }), [focusList]);

  // The first listing of a new connection takes the focus, so the keyboard works at once.
  const focusedOnce = useRef(false);
  useEffect(() => {
    if (focusedOnce.current || !active || !listing.data) return;
    focusedOnce.current = true;
    if (!document.querySelector('dialog[open]')) focusList();
  }, [active, listing.data, focusList]);

  const navigate = useCallback(
    (target: string, from?: string) => {
      pendingCursor.current = from ?? null;
      setFilter('');
      setSelected(new Set());
      setCursor(null);
      anchor.current = null;
      updateConnection(tab.id, { path: target });
      focusList();
    },
    [tab.id, updateConnection, focusList],
  );

  const goUp = useCallback(() => {
    if (path !== '/') navigate(parentPath(path), path);
  }, [navigate, path]);

  const refresh = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: queryKeys.listing(session.id, path) });
  }, [queryClient, session.id, path]);

  const invalidate = useCallback(
    (dirs: string[] = [path]) => {
      for (const dir of new Set(dirs)) {
        void queryClient.invalidateQueries({ queryKey: queryKeys.listing(session.id, dir) });
      }
    },
    [queryClient, session.id, path],
  );

  useShortcut(SHORTCUTS.refresh, active && connected, refresh);

  // Keys inside the filter field: Esc clears it, ↓ and Enter go back to the list.
  useEffect(() => {
    const input = filterInput.current;
    if (!input) return;
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === 'Escape') {
        event.preventDefault();
        event.stopPropagation();
        setFilter('');
        list.current?.element?.focus();
      } else if (event.key === 'ArrowDown' || event.key === 'Enter') {
        event.preventDefault();
        list.current?.element?.focus();
      }
    };
    input.addEventListener('keydown', onKey);
    return () => input.removeEventListener('keydown', onKey);
  }, []);
  // Mod+L reaches the path bar from anywhere, the editor included (CodeMirror would select a line).
  useWindowShortcut(SHORTCUTS.editPath, active, () => pathBar.current?.edit());

  const selection = useMemo(
    () => visible.filter((entry) => selected.has(entry.path)),
    [visible, selected],
  );
  const cursorEntry = cursor ? visible[indexByPath.get(cursor) ?? -1] : undefined;

  const openEntry = (entry: Entry, preview: boolean) => {
    if (isDirLike(entry)) navigate(entry.path);
    else onOpenFile(entry, preview);
  };

  const select = (target: string, mode: 'only' | 'toggle' | 'range') => {
    if (mode === 'only') {
      setSelected(new Set([target]));
      anchor.current = target;
    } else if (mode === 'toggle') {
      setSelected((current) => {
        const next = new Set(current);
        if (next.has(target)) next.delete(target);
        else next.add(target);
        return next;
      });
      anchor.current = target;
    } else {
      const from = indexByPath.get(anchor.current ?? target) ?? 0;
      const to = indexByPath.get(target) ?? 0;
      const [start, end] = from < to ? [from, to] : [to, from];
      setSelected(new Set(visible.slice(start, end + 1).map((entry) => entry.path)));
    }
    setCursor(target);
  };

  const moveCursor = (index: number, extend: boolean) => {
    const entry = visible[Math.max(0, Math.min(visible.length - 1, index))];
    if (!entry) return;
    select(entry.path, extend ? 'range' : 'only');
    list.current?.scrollToIndex(indexByPath.get(entry.path) ?? 0);
  };

  // ---- Actions ----

  const copyPaths = async (entries: Entry[]) => {
    if (!entries.length) return;
    try {
      await navigator.clipboard.writeText(entries.map((entry) => entry.path).join('\n'));
      toast.show({ message: common.copiedPath });
    } catch {
      toast.show({ message: common.copyFailed, tone: 'error' });
    }
  };

  const failure = (cause: unknown): string => errorMessage(cause);

  const download = async (entries: Entry[], chooseFolder: boolean) => {
    if (!entries.length) return;
    try {
      const transfers = await api.downloadPaths(
        session.id,
        entries.map((entry) => entry.path),
        chooseFolder,
      );
      if (transfers.length) {
        useSessionData.getState().upsertTransfers(tab.id, transfers);
        toast.show({ message: t.transfersStarted(entries.length, true) });
      }
    } catch (cause) {
      toast.show({ message: failure(cause), tone: 'error' });
    }
  };

  const upload = async (folders: boolean) => {
    setUploadMenu(null);
    try {
      const transfers = await api.uploadFiles(session.id, path, folders);
      if (transfers.length) {
        useSessionData.getState().upsertTransfers(tab.id, transfers);
        toast.show({ message: t.transfersStarted(transfers.length, false) });
      }
    } catch (cause) {
      toast.show({ message: failure(cause), tone: 'error' });
    }
  };

  /** Open file tabs of removed or renamed paths: clean ones close, dirty ones stay. */
  const releaseTabs = (paths: string[]) => {
    const files = useFileTabs.getState();
    const tabs = files.byConn[tab.id]?.tabs ?? [];
    for (const file of tabs) {
      const gone = paths.some((p) => file.path === p || file.path.startsWith(`${p}/`));
      if (gone && !file.dirty) files.close(tab.id, file.key);
    }
  };

  const createEntry = async (kind: 'newFolder' | 'newFile', name: string) => {
    const target = joinPath(path, name);
    try {
      const entry =
        kind === 'newFolder'
          ? await api.makeDir(session.id, target)
          : await api.makeFile(session.id, target);
      setDialog(null);
      pendingCursor.current = entry.path;
      invalidate();
      if (kind === 'newFile') onOpenFile(entry, false);
      return null;
    } catch (cause) {
      return failure(cause);
    }
  };

  const rename = async (entry: Entry, name: string) => {
    const dirty = (useFileTabs.getState().byConn[tab.id]?.tabs ?? []).some(
      (file) => file.dirty && (file.path === entry.path || file.path.startsWith(`${entry.path}/`)),
    );
    if (dirty) return t.dialogs.saveFirst(entry.name);
    try {
      const renamed = await api.renamePath(session.id, entry.path, joinPath(path, name));
      setDialog(null);
      releaseTabs([entry.path]);
      pendingCursor.current = renamed.path;
      invalidate();
      return null;
    } catch (cause) {
      return failure(cause);
    }
  };

  const chmod = async (entry: Entry, mode: number) => {
    try {
      await api.chmodPath(session.id, entry.path, mode);
      setDialog(null);
      invalidate();
      return null;
    } catch (cause) {
      return failure(cause);
    }
  };

  const remove = async (entries: Entry[]) => {
    setDialog(null);
    try {
      const report = await api.deletePaths(
        session.id,
        entries.map((entry) => entry.path),
      );
      releaseTabs(
        entries.map((entry) => entry.path).filter((p) => !report.failed.some((f) => f.path === p)),
      );
      setSelected(new Set());
      invalidate();
      if (report.failed.length) setDialog({ kind: 'report', report });
      else toast.show({ message: t.dialogs.deleted(report.deleted) });
    } catch (cause) {
      toast.show({ message: failure(cause), tone: 'error' });
    }
    focusList();
  };

  const askDelete = (entries: Entry[]) => {
    if (entries.length && connected) setDialog({ kind: 'delete', entries });
  };

  // ---- Context menu ----

  const menuLabel = (entries: Entry[]) =>
    entries.length === 1
      ? t.menu.label(entries[0]!.name)
      : entries.length
        ? t.menu.labelMany(entries.length)
        : t.list.label(path);

  // The items are built when the menu opens (an event), never during render.
  const openMenu = (x: number, y: number, entries: Entry[], trigger?: HTMLElement) =>
    setMenu({ x, y, trigger, label: menuLabel(entries), items: menuItems(entries) });

  const menuItems = (entries: Entry[]): ContextMenuItem[] => {
    const close = (run: () => void) => () => {
      setMenu(null);
      run();
    };
    const one = entries.length === 1 ? entries[0] : undefined;
    const files = entries.filter((entry) => !isDirLike(entry));
    const write = !connected;
    const items: ContextMenuItem[] = [];
    if (one) {
      items.push({
        label: isDirLike(one) ? t.menu.open : t.menu.preview,
        icon: isDirLike(one) ? <FolderOpen size={13} /> : <Eye size={13} />,
        shortcut: isDirLike(one) ? 'Enter' : 'Space',
        onSelect: close(() => openEntry(one, true)),
      });
      if (!isDirLike(one)) {
        items.push({
          label: t.menu.openNewTab,
          icon: <FilePlus size={13} />,
          shortcut: 'Enter',
          onSelect: close(() => onOpenFile(one, false)),
        });
      }
    } else if (files.length > 1) {
      items.push({
        label: t.menu.openNewTab,
        icon: <FilePlus size={13} />,
        onSelect: close(() => files.forEach((file) => onOpenFile(file, false))),
      });
    }
    if (entries.length) {
      items.push(
        {
          label: t.menu.download,
          icon: <Download size={13} />,
          disabled: write,
          onSelect: close(() => void download(entries, false)),
        },
        {
          label: t.menu.downloadTo,
          icon: <Download size={13} />,
          disabled: write,
          onSelect: close(() => void download(entries, true)),
        },
      );
    }
    if (one) {
      items.push({
        label: t.menu.rename,
        icon: <Pencil size={13} />,
        shortcut: 'F2',
        disabled: write,
        onSelect: close(() => setDialog({ kind: 'rename', entry: one })),
      });
    }
    items.push(
      {
        label: t.menu.newFolder,
        icon: <FolderPlus size={13} />,
        disabled: write,
        onSelect: close(() => setDialog({ kind: 'newFolder' })),
      },
      {
        label: t.menu.newFile,
        icon: <FilePlus size={13} />,
        disabled: write,
        onSelect: close(() => setDialog({ kind: 'newFile' })),
      },
    );
    if (entries.length) {
      items.push({
        label: t.menu.delete,
        icon: <Trash2 size={13} />,
        shortcut: 'Del',
        disabled: write,
        onSelect: close(() => askDelete(entries)),
      });
    }
    if (one && session.capabilities.chmod) {
      items.push({
        label: t.menu.permissions,
        icon: <KeyRound size={13} />,
        disabled: write,
        onSelect: close(() => setDialog({ kind: 'chmod', entry: one })),
      });
    }
    if (entries.length) {
      items.push({
        label: t.menu.copyPath,
        icon: <Copy size={13} />,
        shortcut: keys({ key: 'c', primary: true }),
        onSelect: close(() => void copyPaths(entries)),
      });
    }
    items.push({
      label: t.menu.refresh,
      icon: <RefreshCw size={13} />,
      shortcut: keys(SHORTCUTS.refresh),
      disabled: write,
      onSelect: close(refresh),
    });
    return items;
  };

  // ---- Keyboard ----

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const mod = event.ctrlKey || event.metaKey;
    const index = cursor ? (indexByPath.get(cursor) ?? -1) : -1;
    const page = Math.max(
      1,
      Math.floor((list.current?.element?.clientHeight ?? 540) / ROW_HEIGHT) - 1,
    );
    const handled = () => {
      event.preventDefault();
      event.stopPropagation();
    };
    switch (event.key) {
      case 'ArrowDown':
        handled();
        return moveCursor(index + 1, event.shiftKey);
      case 'ArrowUp':
        handled();
        if (event.altKey) return goUp();
        return moveCursor(index - 1, event.shiftKey);
      case 'Home':
        handled();
        return moveCursor(0, event.shiftKey);
      case 'End':
        handled();
        return moveCursor(visible.length - 1, event.shiftKey);
      case 'PageDown':
        handled();
        return moveCursor(index + page, event.shiftKey);
      case 'PageUp':
        handled();
        return moveCursor(index - page, event.shiftKey);
      case 'Enter':
        handled();
        if (cursorEntry) openEntry(cursorEntry, false);
        return;
      case ' ':
        handled();
        if (cursorEntry && !isDirLike(cursorEntry)) onOpenFile(cursorEntry, true);
        return;
      case 'Backspace':
        handled();
        if (mod) return askDelete(selection.length ? selection : cursorEntry ? [cursorEntry] : []);
        return goUp();
      case 'Delete':
        handled();
        return askDelete(selection.length ? selection : cursorEntry ? [cursorEntry] : []);
      case 'F2':
        handled();
        if (cursorEntry && connected) setDialog({ kind: 'rename', entry: cursorEntry });
        return;
      case 'Escape':
        if (filter) {
          handled();
          setFilter('');
        } else if (selected.size > 1 && cursor) {
          handled();
          setSelected(new Set([cursor]));
        }
        return;
      case 'ContextMenu':
      case 'F10':
        if (event.key === 'F10' && !event.shiftKey) return;
        handled();
        if (cursorEntry) {
          const row = document.getElementById(rowId(tab.id, cursorEntry.path));
          const rect = row?.getBoundingClientRect();
          openMenu(
            (rect?.left ?? 0) + 24,
            rect?.bottom ?? 0,
            selection.length ? selection : [cursorEntry],
            list.current?.element ?? undefined,
          );
        }
        return;
    }
    if (mod && !event.altKey && event.key.toLowerCase() === 'a') {
      handled();
      setSelected(new Set(visible.map((entry) => entry.path)));
      return;
    }
    if (mod && !event.altKey && event.key.toLowerCase() === 'c') {
      handled();
      void copyPaths(selection.length ? selection : cursorEntry ? [cursorEntry] : []);
      return;
    }
    if (mod && !event.altKey && event.key.toLowerCase() === 'f') {
      handled();
      filterInput.current?.focus();
      return;
    }
    // Type-ahead: printable keys go to the filter field.
    if (!mod && !event.altKey && event.key.length === 1) {
      handled();
      setFilter((current) => current + event.key);
      filterInput.current?.focus();
    }
  };

  // ---- Rendering ----

  const onRowClick = (event: MouseEvent, entry: Entry) => {
    if (event.shiftKey) select(entry.path, 'range');
    else if (event.ctrlKey || event.metaKey) select(entry.path, 'toggle');
    else select(entry.path, 'only');
    focusList();
  };

  const onRowContextMenu = (event: MouseEvent, entry: Entry) => {
    event.preventDefault();
    let entries = selection;
    if (!selected.has(entry.path)) {
      select(entry.path, 'only');
      entries = [entry];
    }
    focusList();
    openMenu(event.clientX, event.clientY, entries, list.current?.element ?? undefined);
  };

  const owner = session.capabilities.owner;
  const columns = owner ? COLUMNS : COLUMNS.filter((column) => column.key !== 'owner');

  const renderRow = (index: number, style: CSSProperties) => {
    const entry = visible[index]!;
    const isSelected = selected.has(entry.path);
    const isCursor = cursor === entry.path;
    const dirLike = isDirLike(entry);
    const broken = entry.kind === 'symlink' && entry.targetKind == null;
    return (
      // Keyboard interaction lives on the listbox (active descendant); rows take the pointer.
      // eslint-disable-next-line jsx-a11y-x/click-events-have-key-events
      <div
        key={entry.path}
        id={rowId(tab.id, entry.path)}
        role="option"
        aria-label={entry.name}
        aria-selected={isSelected}
        tabIndex={-1}
        className="gem-file-row"
        data-cursor={isCursor || undefined}
        data-dir={dirLike || undefined}
        data-drop-dir={dirLike ? entry.path : undefined}
        data-drop-target={dropTarget === entry.path || undefined}
        style={style}
        onClick={(event) => onRowClick(event, entry)}
        onDoubleClick={() => openEntry(entry, false)}
        onContextMenu={(event) => onRowContextMenu(event, entry)}
        title={
          entry.kind === 'symlink'
            ? entry.linkTarget
              ? t.list.symlink(entry.linkTarget)
              : t.list.brokenLink
            : undefined
        }
      >
        <span className="gem-file-name" data-broken={broken || undefined}>
          <EntryIcon entry={entry} />
          <span className="gem-file-label">{entry.name}</span>
          {entry.kind === 'symlink' && (
            <span className="gem-file-link" aria-label={t.list.symlink(entry.linkTarget ?? '')}>
              →
            </span>
          )}
        </span>
        <span className="gem-file-size">{dirLike ? '' : formatSize(entry.size)}</span>
        <span className="gem-file-date" title={formatDateTime(entry.mtime)}>
          {formatDate(entry.mtime)}
        </span>
        <span className="gem-file-mode">{formatMode(entry.mode, entry.kind)}</span>
        {owner && (
          <span className="gem-file-owner">
            {entry.owner ?? ''}
            {entry.group && entry.group !== entry.owner ? `:${entry.group}` : ''}
          </span>
        )}
      </div>
    );
  };

  type ListAction = 'up' | 'home' | 'retry' | 'clear';
  const listState = (): {
    title: string;
    hint?: string;
    icon: ReactElement;
    action?: ListAction;
  } | null => {
    if (error && !listing.data) {
      if (error.code === 'permission_denied') {
        return {
          title: t.list.permissionDenied,
          hint: t.list.permissionDeniedHint(path),
          icon: <Lock size={28} aria-hidden />,
          action: path !== '/' ? 'up' : undefined,
        };
      }
      if (error.code === 'not_found') {
        return {
          title: t.list.notFound,
          hint: t.list.notFoundHint(path),
          icon: <FileX size={28} aria-hidden />,
          action: 'home',
        };
      }
      return {
        title: t.list.errorTitle,
        hint: sentence(error.message),
        icon: <FileX size={28} aria-hidden />,
        action: 'retry',
      };
    }
    if (!listing.data) return null;
    if (!visible.length && filter) {
      return {
        title: t.list.noMatch(filter),
        hint: t.list.noMatchHint,
        icon: <FolderOpen size={28} aria-hidden />,
        action: 'clear',
      };
    }
    if (!visible.length) {
      return {
        title: t.list.empty,
        hint: hiddenCount && !showHidden ? t.list.hiddenCount(hiddenCount) : t.list.emptyHint,
        icon: <FolderOpen size={28} aria-hidden />,
      };
    }
    return null;
  };
  const empty = listState();

  return (
    <div
      className="gem-browser"
      data-drop-root={path}
      data-dropping={dropTarget === path || undefined}
    >
      <div className="gem-browser-bar">
        <PathBar
          ref={pathBar}
          path={path}
          label={t.toolbar.pathLabel}
          editLabel={`${t.toolbar.editPath} (${keys(SHORTCUTS.editPath)})`}
          onNavigate={(target) => navigate(target)}
        />
        <div className="gem-toolbar" role="toolbar" aria-label={t.toolbar.label}>
          <IconButton
            aria-label={t.toolbar.up}
            title={`${t.toolbar.up} (Backspace)`}
            disabled={path === '/'}
            onClick={goUp}
          >
            <ArrowUp size={14} aria-hidden />
          </IconButton>
          <IconButton
            aria-label={t.toolbar.refresh}
            title={`${t.toolbar.refresh} (${keys(SHORTCUTS.refresh)})`}
            disabled={!connected}
            onClick={refresh}
          >
            {listing.isFetching && listing.data ? (
              <Spinner size={14} />
            ) : (
              <RefreshCw size={14} aria-hidden />
            )}
          </IconButton>
          <IconButton
            aria-label={t.toolbar.newFolder}
            title={t.toolbar.newFolder}
            disabled={!connected}
            onClick={() => setDialog({ kind: 'newFolder' })}
          >
            <FolderPlus size={14} aria-hidden />
          </IconButton>
          <IconButton
            aria-label={t.toolbar.newFile}
            title={t.toolbar.newFile}
            disabled={!connected}
            onClick={() => setDialog({ kind: 'newFile' })}
          >
            <FilePlus size={14} aria-hidden />
          </IconButton>
          <IconButton
            aria-label={t.toolbar.upload}
            title={t.toolbar.upload}
            aria-haspopup="menu"
            aria-expanded={uploadMenu ? true : undefined}
            disabled={!connected}
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              setUploadMenu({ x: rect.left, y: rect.bottom + 2, trigger: event.currentTarget });
            }}
          >
            <Upload size={14} aria-hidden />
          </IconButton>
          {exec && (
            <IconButton
              aria-label={commandStrings.menu.button}
              title={`${commandStrings.menu.button} (${keys(SHORTCUTS.palette)})`}
              aria-haspopup="menu"
              aria-expanded={snippetMenu ? true : undefined}
              disabled={!connected}
              onClick={(event) => {
                const rect = event.currentTarget.getBoundingClientRect();
                setSnippetMenu({ x: rect.left, y: rect.bottom + 2, trigger: event.currentTarget });
              }}
            >
              <Zap size={14} aria-hidden />
            </IconButton>
          )}
          <IconButton
            aria-label={t.toolbar.showHidden}
            title={showHidden ? t.toolbar.hideHidden : t.toolbar.showHidden}
            aria-pressed={showHidden}
            onClick={() => setShowHidden((value) => !value)}
          >
            {showHidden ? <Eye size={14} aria-hidden /> : <EyeOff size={14} aria-hidden />}
          </IconButton>
        </div>
      </div>
      <div className="gem-browser-filter">
        <SearchField
          label={t.toolbar.filter}
          placeholder={t.toolbar.filterPlaceholder}
          value={filter}
          size={13}
          inputRef={filterInput}
          onChange={setFilter}
        />
      </div>
      <div
        className="gem-file-header"
        data-owner={owner || undefined}
        role="group"
        aria-label={t.columns.sortBy('')}
      >
        {columns.map((column) => {
          const current = sort.key === column.key;
          return (
            <button
              key={column.key}
              type="button"
              className="mnml-button"
              data-column={column.key}
              data-active={current || undefined}
              aria-label={t.columns.sortBy(column.label.toLowerCase())}
              aria-pressed={current}
              onClick={() =>
                setSort((previous) => ({
                  key: column.key,
                  descending: previous.key === column.key ? !previous.descending : false,
                }))
              }
            >
              <span>{column.label}</span>
              {current &&
                (sort.descending ? (
                  <ChevronUp size={11} aria-hidden />
                ) : (
                  <ChevronDown size={11} aria-hidden />
                ))}
            </button>
          );
        })}
      </div>
      {error && listing.data && <Message tone="error">{sentence(error.message)}</Message>}
      <VirtualList
        ref={list}
        count={visible.length}
        rowHeight={ROW_HEIGHT}
        renderRow={renderRow}
        className="gem-file-list"
        data-owner={owner || undefined}
        role="listbox"
        aria-label={t.list.label(path)}
        aria-multiselectable
        aria-activedescendant={
          cursor && indexByPath.has(cursor) ? rowId(tab.id, cursor) : undefined
        }
        aria-busy={listing.isFetching || undefined}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onContextMenu={(event) => {
          // Rows handle their own menu (and prevent the default); this is the empty area.
          if (event.defaultPrevented) return;
          event.preventDefault();
          setSelected(new Set());
          openMenu(event.clientX, event.clientY, [], list.current?.element ?? undefined);
        }}
        empty={
          showLoading ? (
            <div className="gem-list-message">{t.list.loadingSlow}</div>
          ) : empty ? (
            <EmptyState icon={empty.icon} title={empty.title}>
              {empty.hint}
              {empty.action && (
                <span className="gem-empty-action">
                  {empty.action === 'up' && <Button onClick={goUp}>{t.list.goUp}</Button>}
                  {empty.action === 'home' && (
                    <Button onClick={() => navigate(session.home)}>{t.list.goHome}</Button>
                  )}
                  {empty.action === 'retry' && <Button onClick={refresh}>{common.retry}</Button>}
                  {empty.action === 'clear' && (
                    <Button onClick={() => setFilter('')}>{t.list.clearFilter}</Button>
                  )}
                </span>
              )}
            </EmptyState>
          ) : null
        }
      />
      {dropTarget && (
        <div className="gem-drop-hint" aria-live="polite">
          {t.list.dropHere(dropTarget)}
        </div>
      )}

      {menu && (
        <ContextMenu
          label={menu.label}
          className="gem-menu-wide"
          x={menu.x}
          y={menu.y}
          trigger={menu.trigger}
          items={menu.items}
          onDismiss={() => setMenu(null)}
        />
      )}
      {snippetMenu && (
        <ContextMenu
          label={commandStrings.menu.label}
          className="gem-menu-wide"
          x={snippetMenu.x}
          y={snippetMenu.y}
          trigger={snippetMenu.trigger}
          onDismiss={() => setSnippetMenu(null)}
          items={[
            ...(snippets.data ?? []).map((snippet) => ({
              label: snippet.name,
              icon: <SquareTerminal size={13} />,
              // The scope sits where shortcuts go, so globals and site snippets read apart.
              shortcut: snippet.siteId
                ? commandStrings.palette.site
                : commandStrings.palette.global,
              onSelect: () => {
                setSnippetMenu(null);
                launchSnippet(tab.id, snippet, false, useUi.getState().setRunConfirmation);
              },
            })),
            ...(snippets.data?.length
              ? []
              : [{ label: commandStrings.menu.none, disabled: true, onSelect: () => {} }]),
            {
              label: commandStrings.menu.runCommand,
              icon: <Play size={13} />,
              shortcut: keys(SHORTCUTS.palette),
              onSelect: () => {
                setSnippetMenu(null);
                useUi.getState().setPaletteOpen(true);
              },
            },
            {
              label: commandStrings.menu.manage,
              icon: <Settings2 size={13} />,
              onSelect: () => {
                setSnippetMenu(null);
                useUi.getState().openSnippetsDialog({ siteId: tab.siteId, siteName: tab.label });
              },
            },
          ]}
        />
      )}
      {uploadMenu && (
        <ContextMenu
          label={t.toolbar.upload}
          x={uploadMenu.x}
          y={uploadMenu.y}
          trigger={uploadMenu.trigger}
          onDismiss={() => setUploadMenu(null)}
          items={[
            {
              label: t.toolbar.uploadFiles,
              icon: <Upload size={13} />,
              onSelect: () => void upload(false),
            },
            {
              label: t.toolbar.uploadFolder,
              icon: <FolderPlus size={13} />,
              onSelect: () => void upload(true),
            },
          ]}
        />
      )}
      {(dialog?.kind === 'newFolder' || dialog?.kind === 'newFile') && (
        <NameDialog
          title={dialog.kind === 'newFolder' ? t.dialogs.newFolder : t.dialogs.newFile}
          action={t.dialogs.create}
          taken={names}
          onSubmit={(name) => createEntry(dialog.kind as 'newFolder' | 'newFile', name)}
          onCancel={() => {
            setDialog(null);
            focusList();
          }}
        />
      )}
      {dialog?.kind === 'rename' && (
        <NameDialog
          title={t.dialogs.rename(dialog.entry.name)}
          action={t.dialogs.renameAction}
          initial={dialog.entry.name}
          taken={names}
          onSubmit={(name) => rename(dialog.entry, name)}
          onCancel={() => {
            setDialog(null);
            focusList();
          }}
        />
      )}
      {dialog?.kind === 'chmod' && (
        <ChmodDialog
          entry={dialog.entry}
          onSubmit={(mode) => chmod(dialog.entry, mode)}
          onCancel={() => {
            setDialog(null);
            focusList();
          }}
        />
      )}
      {dialog?.kind === 'delete' && (
        <ConfirmDialog
          title={t.dialogs.deleteTitle}
          message={
            dialog.entries.length === 1
              ? t.dialogs.deleteOne(dialog.entries[0]!.name, isDirLike(dialog.entries[0]!))
              : t.dialogs.deleteMany(dialog.entries.length)
          }
          confirmLabel={t.dialogs.deleteConfirm}
          cancelLabel={common.cancel}
          onCancel={() => {
            setDialog(null);
            focusList();
          }}
          onConfirm={() => void remove(dialog.entries)}
        />
      )}
      {dialog?.kind === 'report' && (
        <ErrorDialog
          title={t.dialogs.deleteFailedTitle}
          dismissLabel={common.close}
          message={[
            t.dialogs.deleteFailed(dialog.report.deleted, dialog.report.failed.length),
            ...dialog.report.failed.map((item) => `${item.path}: ${sentence(item.message)}`),
          ].join('\n')}
          onDismiss={() => {
            setDialog(null);
            focusList();
          }}
        />
      )}
    </div>
  );
});
