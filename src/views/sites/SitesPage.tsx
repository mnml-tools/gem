/**
 * The Sites page (plan §3.2): the app's empty state and what Mod+T opens in a new tab. Saved
 * sites (search, keyboard, context menu), quick connect, import and export. Its look is Zap's
 * connections home: a heading line, a raised toolbar, hairline rows and a quiet footer.
 */
import {
  Avatar,
  Badge,
  Button,
  ConfirmDialog,
  ContextMenu,
  EmptyState,
  Message,
  SearchField,
  Separator,
  Spinner,
} from '@mnml-tools/ui';
import { useQueryClient } from '@tanstack/react-query';
import {
  ArrowUpRight,
  ChevronDown,
  Copy,
  FileDown,
  Pencil,
  Plug,
  Plus,
  Server,
  SquareArrowOutUpRight,
  Trash2,
} from 'lucide-react';
import { useId, useRef, useState, type KeyboardEvent } from 'react';
import { useToast } from '@/components/ui/Toast/context';
import { api } from '@/lib/ipc/api';
import type { Site } from '@/lib/ipc/bindings/Site';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import { toIpcError } from '@/lib/ipc/client';
import { errorMessage } from '@/lib/errors';
import { useAppStatus, useSites } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { parseQuickConnect } from '@/lib/quickConnect';
import { matchesSite, PROTOCOL_LABEL, siteAddress, siteColorVar, siteInitial } from '@/lib/sites';
import { useUi } from '@/state/ui';
import { common } from '@/strings/common';
import { sitesStrings as t } from '@/strings/sites';
import { workspaceStrings } from '@/strings/workspace';
import { adhocMeta, connect, siteMeta } from './connectSite';
import { ImportDialog } from './ImportDialog';

type Props = {
  /** The Sites tab this page lives in (connecting replaces it), or `null` with no tabs. */
  tabId: string | null;
};

type Menu = { site: Site; x: number; y: number; trigger?: HTMLElement };

const INSECURE_NOTICE_KEY = 'gem.insecureNotice.';

export function SitesPage({ tabId }: Props) {
  const sites = useSites();
  const status = useAppStatus();
  const queryClient = useQueryClient();
  const toast = useToast();
  const openSiteDialog = useUi((state) => state.openSiteDialog);
  const [search, setSearch] = useState('');
  const [quick, setQuick] = useState('');
  const [quickError, setQuickError] = useState<string | null>(null);
  const [menu, setMenu] = useState<Menu | null>(null);
  const [importMenu, setImportMenu] = useState<{
    x: number;
    y: number;
    trigger: HTMLElement;
  } | null>(null);
  const [imported, setImported] = useState<SiteInput[] | null>(null);
  const [deleting, setDeleting] = useState<Site | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const list = useRef<HTMLDivElement>(null);
  // Several Sites tabs can be mounted at once: ids must be unique.
  const ids = { title: useId(), quick: useId(), quickError: useId() };

  const all = sites.data ?? [];
  const shown = all.filter((site) => matchesSite(site, search));

  const open = (site: Site, newTab: boolean) => {
    setMenu(null);
    // Plain FTP: say once per site that it is not encrypted (plan §3.6).
    if (site.protocol === 'ftp') {
      try {
        if (!localStorage.getItem(INSECURE_NOTICE_KEY + site.id)) {
          localStorage.setItem(INSECURE_NOTICE_KEY + site.id, '1');
          toast.show({ message: common.insecureHint, duration: 8000 });
        }
      } catch {
        /* Without storage the badge alone tells it. */
      }
    }
    connect(siteMeta(site), newTab ? null : tabId);
  };

  const quickConnect = () => {
    const parsed = parseQuickConnect(quick);
    if (!parsed.ok) {
      setQuickError(t.quickConnectErrors[parsed.error]);
      return;
    }
    setQuickError(null);
    setQuick('');
    connect(adhocMeta(parsed.site), tabId, parsed.password);
  };

  const duplicate = async (site: Site) => {
    setMenu(null);
    try {
      await api.duplicateSite(site.id);
      await queryClient.invalidateQueries({ queryKey: queryKeys.sites });
    } catch (cause) {
      toast.show({ message: errorMessage(cause), tone: 'error' });
    }
  };

  const remove = async (site: Site) => {
    setDeleting(null);
    try {
      await api.deleteSite(site.id);
      await queryClient.invalidateQueries({ queryKey: queryKeys.sites });
    } catch (cause) {
      toast.show({ message: errorMessage(cause), tone: 'error' });
    }
  };

  const runImport = async (source: () => Promise<SiteInput[]>) => {
    setImportMenu(null);
    try {
      const found = await source();
      setImported(found);
    } catch (cause) {
      const error = toIpcError(cause);
      if (error.code !== 'cancelled') toast.show({ message: error.message, tone: 'error' });
    }
  };

  const createImported = async (chosen: SiteInput[]): Promise<string | null> => {
    try {
      for (const input of chosen) await api.createSite(input, null);
      await queryClient.invalidateQueries({ queryKey: queryKeys.sites });
      setImported(null);
      toast.show({ message: t.imported(chosen.length) });
      return null;
    } catch (cause) {
      await queryClient.invalidateQueries({ queryKey: queryKeys.sites });
      return errorMessage(cause);
    }
  };

  const exportSites = async () => {
    try {
      if (await api.exportSites()) toast.show({ message: t.exported });
    } catch (cause) {
      toast.show({ message: errorMessage(cause), tone: 'error' });
    }
  };

  const onRowKeyDown = (event: KeyboardEvent<HTMLDivElement>, site: Site) => {
    if (event.target !== event.currentTarget) return;
    const rows = Array.from(list.current?.querySelectorAll<HTMLElement>('.gem-site-row') ?? []);
    const index = rows.indexOf(event.currentTarget);
    const focus = (next: number) => rows[Math.max(0, Math.min(rows.length - 1, next))]?.focus();
    if (event.key === 'ArrowDown') {
      event.preventDefault();
      focus(index + 1);
    } else if (event.key === 'ArrowUp') {
      event.preventDefault();
      focus(index - 1);
    } else if (event.key === 'Home') {
      event.preventDefault();
      focus(0);
    } else if (event.key === 'End') {
      event.preventDefault();
      focus(rows.length - 1);
    } else if (event.key === 'Enter') {
      event.preventDefault();
      open(site, event.ctrlKey || event.metaKey);
    } else if (event.key === 'F2') {
      event.preventDefault();
      openSiteDialog({ mode: 'edit', siteId: site.id });
    } else if (event.key === 'Delete') {
      event.preventDefault();
      setDeleting(site);
    } else if (event.key === 'ContextMenu' || (event.shiftKey && event.key === 'F10')) {
      event.preventDefault();
      const rect = event.currentTarget.getBoundingClientRect();
      setMenu({ site, x: rect.left + 24, y: rect.bottom, trigger: event.currentTarget });
    }
  };

  return (
    <main className="gem-sites" aria-labelledby={ids.title}>
      <div className="gem-sites-heading">
        <div>
          <h1 id={ids.title}>
            {t.title}
            <span className="gem-count">{all.length}</span>
          </h1>
          <p>{t.note}</p>
        </div>
        <div className="gem-sites-actions">
          <Button
            aria-haspopup="menu"
            aria-expanded={importMenu ? true : undefined}
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              setImportMenu({ x: rect.left, y: rect.bottom + 2, trigger: event.currentTarget });
            }}
          >
            <FileDown size={13} aria-hidden />
            {t.import}
            <ChevronDown size={12} aria-hidden />
          </Button>
          <Button disabled={!all.length} onClick={() => void exportSites()}>
            <SquareArrowOutUpRight size={13} aria-hidden />
            {t.export}
          </Button>
          <Button variant="primary" onClick={() => openSiteDialog({ mode: 'new' })}>
            <Plus size={13} aria-hidden />
            {t.newSite}
          </Button>
        </div>
      </div>

      <form
        className="gem-quick-connect"
        onSubmit={(event) => {
          event.preventDefault();
          quickConnect();
        }}
      >
        <label htmlFor={ids.quick}>{t.quickConnect}</label>
        <input
          id={ids.quick}
          value={quick}
          placeholder={t.quickConnectPlaceholder}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={quickError ? true : undefined}
          aria-describedby={quickError ? ids.quickError : undefined}
          onChange={(event) => {
            setQuick(event.target.value);
            setQuickError(null);
          }}
        />
        <Button type="submit" disabled={!quick.trim()}>
          <Plug size={13} aria-hidden />
          {t.quickConnectButton}
        </Button>
        {quickError && (
          <span id={ids.quickError} className="gem-field-error" role="alert">
            {quickError}
          </span>
        )}
      </form>

      <div className="gem-sites-toolbar">
        <SearchField
          label={t.search}
          placeholder={t.searchPlaceholder}
          value={search}
          onChange={setSearch}
          size={14}
        />
        <span>{t.count(all.length)}</span>
      </div>

      {status.data?.dataError && <Message tone="error">{workspaceStrings.dataError}</Message>}
      {sites.error && <Message tone="error">{errorMessage(sites.error)}</Message>}

      {sites.isLoading ? (
        <div className="gem-list-message">
          <Spinner size={14} />
        </div>
      ) : all.length === 0 ? (
        <EmptyState icon={<Server size={28} aria-hidden />} title={t.empty.title}>
          {t.empty.hint}
          <span className="gem-empty-action">
            <Button variant="primary" onClick={() => openSiteDialog({ mode: 'new' })}>
              <Plus size={13} aria-hidden />
              {t.empty.action}
            </Button>
          </span>
        </EmptyState>
      ) : (
        <div className="gem-site-list" ref={list} role="list" aria-label={t.title}>
          {shown.map((site) => (
            <div key={site.id} role="listitem" className="gem-site-item">
              <div
                role="button"
                tabIndex={0}
                className="gem-site-row"
                data-menu={menu?.site.id === site.id || undefined}
                data-selected={selectedId === site.id || undefined}
                aria-label={`${site.name}, ${siteAddress(site)}, ${PROTOCOL_LABEL[site.protocol]}${site.protocol === 'ftp' ? `, ${common.insecure}` : ''}`}
                // One click selects the row, the second click of a double click connects (as
                // Zap opens a connection row). `detail` counts the clicks of the gesture, so this
                // also works where no separate `dblclick` event arrives.
                onClick={(event) => {
                  setSelectedId(site.id);
                  if (event.detail === 2) open(site, false);
                }}
                onFocus={() => setSelectedId(site.id)}
                onKeyDown={(event) => onRowKeyDown(event, site)}
                onContextMenu={(event) => {
                  event.preventDefault();
                  setMenu({
                    site,
                    x: event.clientX,
                    y: event.clientY,
                    trigger: event.currentTarget,
                  });
                }}
              >
                <Avatar color={siteColorVar(site.color)}>{siteInitial(site.name)}</Avatar>
                <div className="gem-site-info">
                  <h3>{site.name}</h3>
                  <p>
                    {siteAddress(site)}
                    {site.initialPath ? ` · ${site.initialPath}` : ''}
                  </p>
                </div>
                {site.protocol === 'ftp' && (
                  <Badge className="gem-badge-insecure">{common.insecure}</Badge>
                )}
                <Badge>{PROTOCOL_LABEL[site.protocol]}</Badge>
                <Button
                  className="gem-site-open"
                  aria-label={t.connect(site.name)}
                  title={t.connect(site.name)}
                  tabIndex={-1}
                  onClick={() => open(site, false)}
                >
                  <ArrowUpRight size={16} aria-hidden />
                </Button>
              </div>
            </div>
          ))}
          {search && !shown.length && <p className="gem-no-results">{t.noMatch(search)}</p>}
        </div>
      )}

      <div className="gem-sites-footer">
        <span>
          <span className="gem-tiny-square" />
          {t.footer.local}
        </span>
        <span>
          {status.data?.keyringAvailable === false ? t.footer.noKeyring : t.footer.keyring}
          <Separator />
          SFTP · FTP · FTPS
        </span>
      </div>

      {menu && (
        <ContextMenu
          label={t.menu.label(menu.site.name)}
          x={menu.x}
          y={menu.y}
          trigger={menu.trigger}
          onDismiss={() => setMenu(null)}
          items={[
            {
              label: t.menu.connect,
              icon: <ArrowUpRight size={13} />,
              shortcut: 'Enter',
              onSelect: () => open(menu.site, false),
            },
            {
              label: t.menu.connectNewTab,
              icon: <SquareArrowOutUpRight size={13} />,
              onSelect: () => open(menu.site, true),
            },
            {
              label: t.menu.edit,
              icon: <Pencil size={13} />,
              shortcut: 'F2',
              onSelect: () => {
                setMenu(null);
                openSiteDialog({ mode: 'edit', siteId: menu.site.id });
              },
            },
            {
              label: t.menu.duplicate,
              icon: <Copy size={13} />,
              onSelect: () => void duplicate(menu.site),
            },
            {
              label: t.menu.delete,
              icon: <Trash2 size={13} />,
              shortcut: 'Del',
              onSelect: () => {
                setMenu(null);
                setDeleting(menu.site);
              },
            },
          ]}
        />
      )}
      {importMenu && (
        <ContextMenu
          label={t.importMenu}
          x={importMenu.x}
          y={importMenu.y}
          trigger={importMenu.trigger}
          onDismiss={() => setImportMenu(null)}
          items={[
            { label: t.importSsh, onSelect: () => void runImport(api.importSshConfig) },
            { label: t.importFilezilla, onSelect: () => void runImport(api.importFilezilla) },
            { label: t.importFile, onSelect: () => void runImport(api.importSitesFile) },
          ]}
        />
      )}
      {imported && (
        <ImportDialog
          sites={imported}
          onImport={createImported}
          onCancel={() => setImported(null)}
        />
      )}
      {deleting && (
        <ConfirmDialog
          title={t.deleteDialog.title}
          message={t.deleteDialog.message(deleting.name)}
          confirmLabel={t.deleteDialog.confirm}
          cancelLabel={common.cancel}
          onCancel={() => setDeleting(null)}
          onConfirm={() => void remove(deleting)}
        />
      )}
    </main>
  );
}
