/**
 * Connection tabs under the title bar (plan §3.1): the kit's tab strip, one tab per session
 * with its state square, Sites tabs, and `+` (Mod+T) after the last tab.
 */
import { Badge, ContextMenu, IconButton, Tab, Tabs } from '@mnml-tools/ui';
import { LayoutGrid, Plus } from 'lucide-react';
import { useState } from 'react';
import { keys, SHORTCUTS } from '@/lib/shortcuts';
import { useSites } from '@/lib/query/hooks';
import { useConnFiles } from '@/state/fileTabs';
import { useUi } from '@/state/ui';
import { tabLabels, useWorkspace, type WorkspaceTab } from '@/state/workspace';
import { common } from '@/strings/common';
import { sitesStrings } from '@/strings/sites';
import { workspaceStrings as t } from '@/strings/workspace';
import { adhocMeta, connect, siteMeta } from '../sites/connectSite';
import { pendingWork } from './closeTab';
import { SessionDot } from './SessionDot';

type Props = { onClose: (id: string) => void };

function tabTitle(tab: WorkspaceTab, labels: Map<string, string>): string {
  return tab.kind === 'sites' ? t.tabs.sites : (labels.get(tab.id) ?? tab.label);
}

function TabItem({
  tab,
  title,
  active,
  onClose,
  onMenu,
}: {
  tab: WorkspaceTab;
  title: string;
  active: boolean;
  onClose: () => void;
  onMenu: (x: number, y: number) => void;
}) {
  const { tabs: files } = useConnFiles(tab.id);
  const dirty = files.some((file) => file.dirty);
  return (
    <Tab
      title={
        <span className="gem-conn-tab-title">
          {title}
          {tab.kind === 'connection' && tab.insecure && (
            <Badge className="gem-badge-insecure">{common.insecure}</Badge>
          )}
        </span>
      }
      icon={
        tab.kind === 'sites' ? (
          <LayoutGrid size={13} aria-hidden />
        ) : (
          <SessionDot status={tab.status} />
        )
      }
      active={active}
      dirty={dirty}
      closeLabel={t.tabs.close(title)}
      onSelect={() => useWorkspace.getState().selectTab(tab.id)}
      onClose={onClose}
      onContextMenu={(event) => {
        event.preventDefault();
        onMenu(event.clientX, event.clientY);
      }}
    />
  );
}

export function ConnectionTabs({ onClose }: Props) {
  const tabs = useWorkspace((state) => state.tabs);
  const activeId = useWorkspace((state) => state.activeId);
  const sites = useSites();
  const openSiteDialog = useUi((state) => state.openSiteDialog);
  const [menu, setMenu] = useState<{ id: string; x: number; y: number } | null>(null);
  const menuTab = menu ? tabs.find((tab) => tab.id === menu.id) : undefined;
  const labels = tabLabels(tabs);

  const duplicate = (tab: WorkspaceTab) => {
    if (tab.kind !== 'connection') return;
    const site = tab.siteId ? sites.data?.find((item) => item.id === tab.siteId) : undefined;
    if (site) connect(siteMeta(site), null);
    else if (tab.target.kind === 'adhoc') connect(adhocMeta(tab.target.site), null);
  };

  return (
    <nav className="gem-conn-tabs" aria-label={t.tabs.label}>
      <Tabs>
        {tabs.map((tab) => (
          <TabItem
            key={tab.id}
            tab={tab}
            title={tabTitle(tab, labels)}
            active={tab.id === activeId}
            onClose={() => onClose(tab.id)}
            onMenu={(x, y) => setMenu({ id: tab.id, x, y })}
          />
        ))}
        <IconButton
          className="gem-new-tab"
          aria-label={t.tabs.newTab}
          title={`${t.tabs.newTab} (${keys(SHORTCUTS.newTab)})`}
          onClick={() => useWorkspace.getState().openSitesTab()}
        >
          <Plus size={14} aria-hidden />
        </IconButton>
      </Tabs>
      {menu && menuTab && (
        <ContextMenu
          label={t.tabs.menu(tabTitle(menuTab, labels))}
          x={menu.x}
          y={menu.y}
          onDismiss={() => setMenu(null)}
          items={[
            ...(menuTab.kind === 'connection'
              ? [
                  {
                    label: t.tabs.duplicate,
                    onSelect: () => {
                      setMenu(null);
                      duplicate(menuTab);
                    },
                  },
                  ...(menuTab.target.kind === 'adhoc'
                    ? [
                        {
                          label: sitesStrings.saveAsSite,
                          onSelect: () => {
                            setMenu(null);
                            if (menuTab.target.kind === 'adhoc') {
                              openSiteDialog({ mode: 'new', initial: menuTab.target.site });
                            }
                          },
                        },
                      ]
                    : menuTab.siteId
                      ? [
                          {
                            label: sitesStrings.menu.edit,
                            onSelect: () => {
                              setMenu(null);
                              openSiteDialog({ mode: 'edit', siteId: menuTab.siteId! });
                            },
                          },
                        ]
                      : []),
                ]
              : []),
            {
              label: t.tabs.closeTab,
              shortcut: keys(SHORTCUTS.closeTab),
              onSelect: () => {
                setMenu(null);
                onClose(menuTab.id);
              },
            },
            {
              label: t.tabs.closeOthers,
              disabled: tabs.length < 2,
              onSelect: () => {
                setMenu(null);
                // Tabs with unsaved files or running transfers stay open (they would ask).
                for (const tab of tabs) {
                  const work = pendingWork(tab.id);
                  if (tab.id !== menuTab.id && !work.dirty && !work.transfers) onClose(tab.id);
                }
              },
            },
          ]}
        />
      )}
    </nav>
  );
}
