/**
 * The command palette (Mod+K, ADR 0014 proposal C), with Zap's Go To look: a fuzzy filter over
 * this site's and the global snippets and the tab's recent free commands; free text offers
 * "Run <text>" first. Enter runs (reusing a finished tab of the same command), Mod+Enter runs in
 * a new tab, Esc closes. Without an SSH connection it only explains why.
 */
import { Badge, HighlightedText } from '@mnml-tools/ui';
import { History, Play, Search, SquareTerminal, TerminalSquare } from 'lucide-react';
import { useEffect, useId, useMemo, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import { fuzzyMatch } from '@/lib/fuzzy';
import type { Snippet } from '@/lib/ipc/bindings/Snippet';
import { useSnippets } from '@/lib/query/hooks';
import { MOD_ENTER } from '@/lib/shortcuts';
import { useUi } from '@/state/ui';
import type { ConnectionTab } from '@/state/workspace';
import { commandStrings } from '@/strings/commands';
import { common } from '@/strings/common';
import { launchSnippet, recentCommands, startCommand } from './runner';

const t = commandStrings.palette;

type Item =
  | { kind: 'free'; key: string; command: string }
  | { kind: 'snippet'; key: string; snippet: Snippet; rank: number; nameMatch: number[] }
  | { kind: 'recent'; key: string; command: string; rank: number; matches: number[] };

function useOutside(ref: React.RefObject<HTMLElement | null>, onOutside: () => void) {
  useEffect(() => {
    const outside = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) onOutside();
    };
    window.addEventListener('pointerdown', outside);
    return () => window.removeEventListener('pointerdown', outside);
  }, [ref, onOutside]);
}

function Shell({
  id,
  label,
  onClose,
  children,
}: {
  id: string;
  label: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useOutside(ref, onClose);
  const width = Math.min(640, window.innerWidth - 24);
  return createPortal(
    <div
      ref={ref}
      id={id}
      className="gem-palette"
      role="dialog"
      aria-label={label}
      style={{ width, left: (window.innerWidth - width) / 2 }}
    >
      {children}
    </div>,
    document.body,
  );
}

export function CommandPalette({ tab }: { tab: ConnectionTab | null }) {
  const close = useUi((state) => state.setPaletteOpen);
  const onClose = () => close(false);
  const id = useId();
  const exec = !!tab?.session?.capabilities.exec && tab.status === 'connected';
  if (!tab || !exec) return <Unavailable id={id} onClose={onClose} />;
  return <Palette id={id} tab={tab} onClose={onClose} />;
}

function Unavailable({ id, onClose }: { id: string; onClose: () => void }) {
  const button = useRef<HTMLButtonElement>(null);
  useEffect(() => button.current?.focus(), []);
  return (
    <Shell id={id} label={t.label} onClose={onClose}>
      <div className="gem-palette-unavailable" role="status">
        <TerminalSquare size={20} aria-hidden />
        <strong>{t.unavailable}</strong>
        <span>{t.unavailableHint}</span>
        <button
          ref={button}
          type="button"
          className="mnml-button"
          onClick={onClose}
          onKeyDown={(event) => {
            if (event.key === 'Escape') onClose();
          }}
        >
          {common.close}
        </button>
      </div>
    </Shell>
  );
}

function Palette({ id, tab, onClose }: { id: string; tab: ConnectionTab; onClose: () => void }) {
  const [search, setSearch] = useState('');
  const [activeIndex, setActiveIndex] = useState(0);
  const input = useRef<HTMLInputElement>(null);
  const returnFocus = useRef<Element | null>(
    typeof document === 'undefined' ? null : document.activeElement,
  );
  const snippets = useSnippets(tab.siteId, true);
  const confirm = useUi((state) => state.setRunConfirmation);
  const recent = recentCommands(tab.id);

  useEffect(() => {
    input.current?.focus();
  }, []);

  // Matches come first, best one selected, so Enter on a partial name runs that snippet; the
  // typed text itself ("Run …") is last, or first when nothing matches. A recent command equal
  // to the text ranks above everything (and makes "Run …" redundant).
  const items = useMemo<Item[]>(() => {
    const text = search.trim();
    const matched: Item[] = [];
    for (const snippet of snippets.data ?? []) {
      const byName = fuzzyMatch(snippet.name, text);
      const byCommand = byName ? null : fuzzyMatch(snippet.command, text);
      const match = byName ?? (byCommand ? { rank: byCommand.rank + 1, indices: [] } : null);
      if (match) {
        matched.push({
          kind: 'snippet',
          key: `snippet:${snippet.id}`,
          snippet,
          rank: match.rank,
          nameMatch: byName?.indices ?? [],
        });
      }
    }
    let exact = false;
    for (const command of recent) {
      const match = fuzzyMatch(command, text);
      if (!match) continue;
      exact ||= !!text && command === text;
      matched.push({
        kind: 'recent',
        key: `recent:${command}`,
        command,
        rank: text && command === text ? -1 : match.rank + 0.5,
        matches: match.indices,
      });
    }
    // Stable: snippets keep their saved order among equal ranks.
    matched.sort((a, b) => ('rank' in a && 'rank' in b ? a.rank - b.rank : 0));
    if (!text || exact) return matched;
    return [...matched, { kind: 'free', key: 'free', command: text }];
  }, [search, snippets.data, recent]);

  const active = Math.min(activeIndex, items.length - 1);

  useEffect(() => {
    if (active >= 0)
      document.getElementById(`${id}-option-${active}`)?.scrollIntoView?.({ block: 'nearest' });
  }, [active, id]);

  const dismiss = (restore: boolean) => {
    onClose();
    if (restore && returnFocus.current instanceof HTMLElement && returnFocus.current.isConnected) {
      returnFocus.current.focus();
    }
  };

  const choose = (item: Item, newTab: boolean) => {
    dismiss(false);
    if (item.kind === 'snippet') launchSnippet(tab.id, item.snippet, newTab, confirm);
    else {
      startCommand(tab.id, { command: item.command, title: item.command, snippetId: null }, newTab);
    }
  };

  return (
    <Shell id={id} label={t.label} onClose={() => dismiss(false)}>
      <div className="gem-palette-body">
        <div className="gem-palette-toolbar">
          <SquareTerminal size={13} aria-hidden />
          <span>{t.label}</span>
          <span className="gem-palette-site" title={tab.label}>
            {tab.label}
          </span>
        </div>
        <label className="gem-palette-search">
          <Search size={14} aria-hidden />
          <input
            ref={input}
            role="combobox"
            aria-label={t.label}
            aria-autocomplete="list"
            aria-expanded="true"
            aria-controls={`${id}-results`}
            aria-activedescendant={active >= 0 ? `${id}-option-${active}` : undefined}
            placeholder={t.placeholder}
            spellCheck={false}
            autoComplete="off"
            value={search}
            onChange={(event) => {
              setSearch(event.target.value);
              setActiveIndex(0);
            }}
            onKeyDown={(event) => {
              if (event.nativeEvent.isComposing) return;
              if (event.key === 'ArrowDown' || event.key === 'ArrowUp') {
                event.preventDefault();
                if (items.length) {
                  setActiveIndex(
                    (event.key === 'ArrowDown' ? active + 1 : active - 1 + items.length) %
                      items.length,
                  );
                }
              } else if (event.key === 'Enter') {
                event.preventDefault();
                const item = items[active];
                if (item && !event.repeat) choose(item, event.ctrlKey || event.metaKey);
              } else if (event.key === 'Escape') {
                event.preventDefault();
                event.stopPropagation();
                dismiss(true);
              }
            }}
          />
        </label>
        <div
          id={`${id}-results`}
          className="gem-palette-results"
          role="listbox"
          aria-label={t.label}
        >
          {items.map((item, index) => (
            <button
              key={item.key}
              type="button"
              id={`${id}-option-${index}`}
              role="option"
              aria-selected={active === index}
              tabIndex={-1}
              className="gem-palette-option"
              onPointerMove={() => setActiveIndex(index)}
              onMouseDown={(event) => event.preventDefault()}
              onClick={(event) => choose(item, event.ctrlKey || event.metaKey)}
            >
              {item.kind === 'free' && (
                <>
                  <Play size={13} aria-hidden />
                  <span className="gem-palette-label">{t.run(item.command)}</span>
                </>
              )}
              {item.kind === 'snippet' && (
                <>
                  <SquareTerminal size={13} aria-hidden />
                  <HighlightedText
                    className="gem-palette-label"
                    text={item.snippet.name}
                    indices={item.nameMatch}
                  />
                  <span className="gem-palette-detail">{item.snippet.command}</span>
                  <Badge className="gem-palette-scope">
                    {item.snippet.siteId ? t.site : t.global}
                  </Badge>
                </>
              )}
              {item.kind === 'recent' && (
                <>
                  <History size={13} aria-hidden />
                  <HighlightedText
                    className="gem-palette-label"
                    text={item.command}
                    indices={item.matches}
                  />
                  <span className="gem-palette-detail">{t.recent}</span>
                </>
              )}
            </button>
          ))}
        </div>
        <p className="gem-palette-footer" role="status">
          {items.length === 0
            ? snippets.data?.length
              ? t.empty
              : t.noSnippets
            : t.hint(MOD_ENTER)}
        </p>
      </div>
    </Shell>
  );
}
