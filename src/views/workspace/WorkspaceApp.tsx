/**
 * The window (plan §3.1): the kit's title bar, the connection tabs, every tab's body (kept
 * mounted while hidden), the bottom panel of the active connection, and the status bar.
 * Window-wide shortcuts, the close guards and the window-level dialogs live here.
 */
import {
  AppShell,
  Badge,
  Brand,
  ConfirmDialog,
  IconButton,
  registerShortcut,
  Separator,
  StatusBar,
  ThemeToggle,
  TitleBar,
  useShortcut,
} from '@mnml-tools/ui';
import { WindowControls } from '@mnml-tools/ui/tauri';
import { useQueryClient } from '@tanstack/react-query';
import { Settings as SettingsIcon } from 'lucide-react';
import { BrandGlyph } from './BrandGlyph';
import { useCallback, useEffect, useState } from 'react';
import { drawsResizeEdges, drawsWindowControls, macInset } from '@/app/platform';
import { theme } from '@/app/theme';
import { SplitPane } from '@/components/ui/SplitPane/SplitPane';
import { ToastProvider } from '@/components/ui/Toast/Toast';
import { installDialogGuard } from '@/lib/dialogGuard';
import { formatSpeed } from '@/lib/format';
import { useSettings, useSites } from '@/lib/query/hooks';
import { queryKeys } from '@/lib/query/keys';
import { useWindowShortcut } from '@/lib/hooks/useWindowShortcut';
import { keys, SHORTCUTS, TAB_NUMBER_SHORTCUTS } from '@/lib/shortcuts';
import { PROTOCOL_LABEL } from '@/lib/sites';
import { transferSummary, useSessionData, useTransfers } from '@/state/sessionData';
import { useUi } from '@/state/ui';
import { closeActiveFileTab } from '@/state/viewerCommands';
import {
  tabLabels,
  useActiveConnection,
  useTabLabel,
  useWorkspace,
  type ConnectionTab,
} from '@/state/workspace';
import { common } from '@/strings/common';
import { commandStrings } from '@/strings/commands';
import { panelStrings } from '@/strings/panel';
import { workspaceStrings as t } from '@/strings/workspace';
import { CommandPalette } from '../commands/CommandPalette';
import { onRunEnd, snippetSpec, startCommand } from '../commands/runner';
import { SnippetsDialog } from '../commands/SnippetsDialog';
import { ConnectionView } from '../connection/ConnectionView';
import { BottomPanel } from '../panel/BottomPanel';
import { SettingsDialog } from '../settings/SettingsDialog';
import { ShortcutsDialog } from '../settings/ShortcutsDialog';
import { SiteDialog } from '../sites/SiteDialog';
import { SitesPage } from '../sites/SitesPage';
import { closeTabNow, pendingWork } from './closeTab';
import { ConnectionTabs } from './ConnectionTabs';
import { SessionDot } from './SessionDot';
import { useWindowCloseGuard } from './useWindowCloseGuard';
import { ResizeEdges } from './WindowChrome';

type CloseRequest =
  | { kind: 'tab'; id: string; label: string; dirty: number; transfers: number }
  | { kind: 'window'; dirty: number; transfers: number; confirm: () => void };

function TitleCenter({ tab }: { tab: ConnectionTab | null }) {
  const label = useTabLabel(tab);
  if (!tab) return <div className="gem-title-center">{t.tabs.sites}</div>;
  return (
    <div className="gem-title-center">
      <strong>{label}</strong>
      <span className="gem-title-protocol">{PROTOCOL_LABEL[tab.protocol]}</span>
      {tab.insecure && <Badge className="gem-badge-insecure">{common.insecure}</Badge>}
    </div>
  );
}

function WorkspaceStatusBar({ tab }: { tab: ConnectionTab | null }) {
  const transfers = useTransfers(tab?.id ?? null);
  const speeds = useSessionData((state) => state.speeds);
  const sites = useSites();
  const label = useTabLabel(tab);
  if (!tab) {
    const count = sites.data?.length ?? 0;
    return (
      <StatusBar
        start={`GEM / ${t.tabs.sites}`}
        end={`${count} ${count === 1 ? 'site' : 'sites'}`}
      />
    );
  }
  const summary = transferSummary(transfers, speeds);
  return (
    <StatusBar
      start={
        <span className="gem-status-start">
          GEM / {label}
          {tab.path && (
            <>
              <Separator />
              {tab.path}
            </>
          )}
        </span>
      }
      end={
        <span>
          {tab.items != null && (
            <>
              {t.statusBar.items(tab.items)}
              <Separator />
            </>
          )}
          ↑ {summary.uploads} ↓ {summary.downloads}
          <Separator />
          {summary.rate > 0 ? formatSpeed(summary.rate) : t.statusBar.idle}
        </span>
      }
    />
  );
}

export default function WorkspaceApp() {
  return (
    <ToastProvider>
      <Workspace />
    </ToastProvider>
  );
}

function Workspace() {
  const tabs = useWorkspace((state) => state.tabs);
  const activeId = useWorkspace((state) => state.activeId);
  const active = useActiveConnection();
  const panelOpen = useSessionData((state) => state.panelOpen);
  const ui = useUi();
  const sites = useSites();
  // Fetched early: new tabs read "show hidden files" and the size thresholds from it.
  useSettings();
  const queryClient = useQueryClient();
  const [closing, setClosing] = useState<CloseRequest | null>(null);

  const requestClose = useCallback(
    (id: string) => {
      const tab = useWorkspace.getState().tabs.find((item) => item.id === id);
      if (!tab) return;
      const work = pendingWork(id);
      if (tab.kind === 'connection' && (work.dirty || work.transfers)) {
        setClosing({
          kind: 'tab',
          id,
          label: tabLabels(useWorkspace.getState().tabs).get(id) ?? tab.label,
          ...work,
        });
      } else closeTabNow(id, queryClient);
    },
    [queryClient],
  );

  // A finished command may have created or removed files: refetch the folders it touched, of
  // that session only. The listing keeps its selection (component state) and the focus.
  useEffect(
    () =>
      onRunEnd((sessionId, dirs) => {
        for (const dir of dirs) {
          void queryClient.invalidateQueries({ queryKey: queryKeys.listing(sessionId, dir) });
        }
      }),
    [queryClient],
  );

  // Dialogs that open mid-gesture (a host-key prompt after Connect) ignore that gesture.
  useEffect(() => installDialogGuard(), []);

  useWindowCloseGuard((confirm) => {
    const work = pendingWork();
    if (!work.dirty && !work.transfers) return false;
    setClosing({ kind: 'window', ...work, confirm });
    return true;
  });

  // ---- Window-wide shortcuts ----
  const noDialog =
    !closing &&
    !ui.settingsOpen &&
    !ui.helpOpen &&
    !ui.siteDialog &&
    !ui.snippetsDialog &&
    !ui.runConfirmation;
  useShortcut(SHORTCUTS.newTab, noDialog, () => useWorkspace.getState().openSitesTab());
  // Mod+W closes the active file tab when the focus is in the viewer, else the connection tab.
  // Captured before the editor, which would otherwise swallow it.
  useWindowShortcut(SHORTCUTS.closeTab, noDialog && !!activeId, () => {
    if (!activeId) return;
    const inViewer = document.activeElement?.closest('.gem-tab-body:not([hidden]) .gem-viewer');
    if (inViewer && closeActiveFileTab(activeId)) return;
    requestClose(activeId);
  });
  useShortcut(SHORTCUTS.togglePanel, !!active, () => useSessionData.getState().togglePanel());
  useShortcut(SHORTCUTS.help, true, () => ui.setHelpOpen(!useUi.getState().helpOpen));
  useShortcut(SHORTCUTS.settings, noDialog, () => ui.setSettingsOpen(true));
  useShortcut(SHORTCUTS.palette, noDialog && !ui.paletteOpen, () => ui.setPaletteOpen(true));

  useEffect(() => {
    const stops = TAB_NUMBER_SHORTCUTS.map((shortcut, index) =>
      registerShortcut({
        shortcut,
        enabled: () => useWorkspace.getState().tabs.length > index,
        run: () => {
          const tab = useWorkspace.getState().tabs[index];
          if (tab) useWorkspace.getState().selectTab(tab.id);
        },
      }),
    );
    // Ctrl+Tab cycles on every platform (it is Ctrl, not ⌘, on macOS too).
    const cycle = (event: KeyboardEvent) => {
      if (!event.ctrlKey || event.key !== 'Tab' || event.altKey || event.metaKey) return;
      if (document.querySelector('dialog[open]')) return;
      event.preventDefault();
      useWorkspace.getState().cycleTabs(event.shiftKey ? -1 : 1);
    };
    document.addEventListener('keydown', cycle, true);
    return () => {
      stops.forEach((stop) => stop());
      document.removeEventListener('keydown', cycle, true);
    };
  }, []);

  const siteBeingEdited =
    ui.siteDialog?.mode === 'edit'
      ? (sites.data?.find((site) => site.id === (ui.siteDialog as { siteId: string }).siteId) ??
        null)
      : null;

  const status = active?.status ?? 'idle';
  const statusText =
    status === 'idle'
      ? t.status.idle
      : status === 'connecting' && active?.session
        ? t.status.connecting
        : t.status[status];

  return (
    <AppShell framed={drawsResizeEdges}>
      <TitleBar
        macInset={macInset}
        brand={<Brand icon={<BrandGlyph />} name={common.appName} note={common.appNote} />}
        center={<TitleCenter tab={active} />}
        end={
          <>
            <ThemeToggle theme={theme} />
            <IconButton
              className="gem-titlebar-button"
              data-tauri-drag-region="false"
              aria-label={t.settings}
              title={`${t.settings} (${keys(SHORTCUTS.settings)})`}
              aria-haspopup="dialog"
              aria-expanded={ui.settingsOpen}
              onClick={() => ui.setSettingsOpen(true)}
            >
              <SettingsIcon size={14} aria-hidden />
            </IconButton>
            <SessionDot status={status} />
            <span className="gem-title-status">{statusText}</span>
            {drawsWindowControls && <WindowControls />}
          </>
        }
      />
      {tabs.length > 0 && <ConnectionTabs onClose={requestClose} />}
      <SplitPane
        className="gem-main"
        storageKey="gem.panelHeight"
        label={panelStrings.resize}
        initial={190}
        minimum={90}
        maximum={720}
        collapsed={!panelOpen}
        top={
          <>
            {tabs.length === 0 && <SitesPage tabId={null} />}
            {tabs.map((tab) => (
              <section
                key={tab.id}
                className="gem-tab-body"
                hidden={tab.id !== activeId}
                aria-label={tab.kind === 'sites' ? t.tabs.sites : tab.label}
              >
                {tab.kind === 'sites' ? (
                  <SitesPage tabId={tab.id} />
                ) : (
                  <ConnectionView
                    tab={tab}
                    active={tab.id === activeId}
                    onEditSite={(siteId) => ui.openSiteDialog({ mode: 'edit', siteId })}
                  />
                )}
              </section>
            ))}
          </>
        }
        bottom={active ? <BottomPanel tab={active} /> : null}
      />
      <WorkspaceStatusBar tab={active} />
      <ResizeEdges />

      {ui.settingsOpen && <SettingsDialog onClose={() => ui.setSettingsOpen(false)} />}
      {ui.helpOpen && <ShortcutsDialog onClose={() => ui.setHelpOpen(false)} />}
      {ui.paletteOpen && <CommandPalette tab={active} />}
      {ui.snippetsDialog && (
        <SnippetsDialog
          siteId={ui.snippetsDialog.siteId}
          siteName={ui.snippetsDialog.siteName}
          onClose={() => ui.openSnippetsDialog(null)}
        />
      )}
      {ui.runConfirmation && (
        <ConfirmDialog
          title={commandStrings.confirmRun.title}
          message={
            <>
              {commandStrings.confirmRun.message(ui.runConfirmation.snippet.name)}
              <code className="gem-confirm-command">{ui.runConfirmation.preview}</code>
            </>
          }
          confirmLabel={commandStrings.confirmRun.confirm}
          cancelLabel={common.cancel}
          onCancel={() => ui.setRunConfirmation(null)}
          onConfirm={() => {
            const request = ui.runConfirmation!;
            ui.setRunConfirmation(null);
            startCommand(request.connId, snippetSpec(request.snippet), request.newTab);
          }}
        />
      )}
      {ui.siteDialog && (ui.siteDialog.mode === 'new' || siteBeingEdited) && (
        <SiteDialog
          key={ui.siteDialog.mode === 'edit' ? ui.siteDialog.siteId : 'new'}
          site={siteBeingEdited}
          initial={ui.siteDialog.mode === 'new' ? ui.siteDialog.initial : undefined}
          onSaved={() => ui.openSiteDialog(null)}
          onCancel={() => ui.openSiteDialog(null)}
        />
      )}
      {closing?.kind === 'tab' && (
        <ConfirmDialog
          title={t.closeConnection.title}
          message={[
            closing.dirty ? t.closeConnection.dirty(closing.label, closing.dirty) : '',
            closing.transfers ? t.closeConnection.transfers(closing.transfers) : '',
          ]
            .filter(Boolean)
            .join(' ')}
          confirmLabel={t.closeConnection.confirm}
          cancelLabel={common.cancel}
          onCancel={() => setClosing(null)}
          onConfirm={() => {
            setClosing(null);
            closeTabNow(closing.id, queryClient);
          }}
        />
      )}
      {closing?.kind === 'window' && (
        <ConfirmDialog
          title={t.closeWindow.title}
          message={[
            closing.dirty ? t.closeWindow.dirty(closing.dirty) : '',
            closing.transfers ? t.closeWindow.transfers(closing.transfers) : '',
          ]
            .filter(Boolean)
            .join(' ')}
          confirmLabel={t.closeWindow.confirm}
          cancelLabel={common.cancel}
          onCancel={() => setClosing(null)}
          onConfirm={() => {
            setClosing(null);
            closing.confirm();
          }}
        />
      )}
    </AppShell>
  );
}
