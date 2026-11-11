/**
 * A command output tab (ADR 0014, proposal B): the command as sent, its state (running with the
 * elapsed time, exit status, stopped, failed), Stop / Re-run / Copy output, and the output in a
 * read-only CodeView. Output is appended in place, batched per animation frame; stderr ranges
 * are marked; the view follows new output until the user scrolls up.
 */
import { Button, IconButton, Separator, Spinner } from '@mnml-tools/ui';
import { StateEffect, StateField, type Extension } from '@codemirror/state';
import { Decoration, EditorView, type DecorationSet } from '@codemirror/view';
import { Check, CircleAlert, Copy, Play, RotateCcw, Square } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { CodeView, type CodeViewHandle } from '@/components/ui/CodeView/CodeView';
import { useToast } from '@/components/ui/Toast/context';
import type { Range } from '@/lib/commandOutput';
import { formatSize } from '@/lib/format';
import { isLanguageId, LANGUAGE_OPTIONS } from '@/lib/languages';
import { useFileTabs, type FileTab } from '@/state/fileTabs';
import { common } from '@/strings/common';
import { commandStrings as t } from '@/strings/commands';
import { useLanguageSupport } from '../viewer/useLanguageSupport';
import { commandOutput, runInTab, stopCommand } from './runner';

const addStderr = StateEffect.define<Range[]>();
const stderrMark = Decoration.mark({ class: 'cm-gem-stderr' });

const marks = (ranges: Range[]) =>
  ranges
    .filter((range) => range.to > range.from)
    .map((range) => stderrMark.range(range.from, range.to));

/** Stderr ranges of the document, kept in place as output is trimmed and appended. */
function stderrField(initial: Range[]): Extension {
  return StateField.define<DecorationSet>({
    create: () => Decoration.set(marks(initial), true),
    update(value, tr) {
      let next = value.map(tr.changes);
      for (const effect of tr.effects) {
        if (effect.is(addStderr)) next = next.update({ add: marks(effect.value), sort: true });
      }
      return next;
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}

function formatDuration(ms: number): string {
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`;
  const seconds = ms / 1000;
  if (seconds < 60) return `${seconds < 10 ? seconds.toFixed(1) : Math.round(seconds)} s`;
  const minutes = Math.floor(seconds / 60);
  return `${minutes} min ${Math.round(seconds % 60)} s`;
}

/** Re-renders every second while `on`. */
function useTicker(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [on]);
  return now;
}

type Props = {
  connId: string;
  tab: FileTab;
  connected: boolean;
  wrap: boolean;
  onEscape: () => void;
};

export function CommandPane({ connId, tab, connected, wrap, onEscape }: Props) {
  const info = tab.command!;
  const toast = useToast();
  const running = info.state === 'running';
  const now = useTicker(running);
  const output = commandOutput(connId, tab.key);
  const [dropped, setDropped] = useState(output.dropped);
  const [size, setSize] = useState(output.text.length);
  const language = useLanguageSupport(tab.language);
  const sent = info.sent ?? info.command;
  const elapsed = running ? now - info.startedAt : (info.durationMs ?? now - info.startedAt);

  const copy = async (text: string, message: string) => {
    try {
      await navigator.clipboard.writeText(text);
      toast.show({ message });
    } catch {
      toast.show({ message: common.copyFailed, tone: 'error' });
    }
  };

  const status = () => {
    if (running) {
      return (
        <span className="gem-command-state" data-state="running">
          <Spinner size={12} />
          {t.running} · {formatDuration(elapsed)}
        </span>
      );
    }
    if (info.state === 'failed') {
      return (
        <span className="gem-command-state" data-state="failed" title={info.error ?? undefined}>
          <CircleAlert size={12} aria-hidden />
          {t.failed}
          {info.error ? `: ${info.error}` : ''}
        </span>
      );
    }
    if (info.state === 'stopped') {
      return (
        <span className="gem-command-state" data-state="stopped">
          <Square size={11} aria-hidden />
          {t.stopped} · {formatDuration(elapsed)}
        </span>
      );
    }
    const code = info.exit?.code;
    const ok = code === 0;
    return (
      <span className="gem-command-state" data-state={ok ? 'ok' : 'error'}>
        {ok ? <Check size={12} aria-hidden /> : <CircleAlert size={12} aria-hidden />}
        {info.exit?.signal && code == null ? t.signal(info.exit.signal) : t.exit(code ?? -1)}
        {' · '}
        {formatDuration(elapsed)}
      </span>
    );
  };

  return (
    <div className="gem-command">
      <div className="gem-command-bar">
        <Play size={12} aria-hidden className="gem-command-icon" />
        <code className="gem-command-text" title={sent}>
          {sent}
        </code>
        <IconButton
          aria-label={t.copyCommand}
          title={t.copyCommand}
          onClick={() => void copy(sent, t.copiedCommand)}
        >
          <Copy size={12} aria-hidden />
        </IconButton>
        <span className="gem-command-spacer" />
        {status()}
        {running ? (
          <Button onClick={() => void stopCommand(connId, tab.key)}>
            <Square size={11} aria-hidden />
            {t.stop}
          </Button>
        ) : (
          <Button
            disabled={!connected}
            title={connected ? undefined : t.reconnectFirst}
            onClick={() => void runInTab(connId, tab.key)}
          >
            <RotateCcw size={12} aria-hidden />
            {t.rerun}
          </Button>
        )}
        <Button onClick={() => void copy(commandOutput(connId, tab.key).text, t.copiedOutput)}>
          <Copy size={12} aria-hidden />
          {t.copyOutput}
        </Button>
      </div>
      {dropped > 0 && (
        <div className="gem-command-notice" role="status">
          {t.dropped(formatSize(output.limit * 0.9), formatSize(dropped))}
        </div>
      )}
      <OutputView
        // A re-run starts a fresh document.
        key={`${tab.key}:${info.generation}`}
        connId={connId}
        tab={tab}
        language={language}
        wrap={wrap}
        onEscape={onEscape}
        onSynced={(next) => {
          setDropped(next.dropped);
          setSize(next.size);
        }}
      />
      <footer className="gem-viewer-footer" aria-label={t.footerLabel}>
        <div className="gem-viewer-footer-start">
          <select
            className="gem-language-picker"
            aria-label={t.language}
            value={tab.language}
            onChange={(event) => {
              if (isLanguageId(event.target.value)) {
                useFileTabs.getState().update(connId, tab.key, {
                  language: event.target.value,
                  languageManual: true,
                });
              }
            }}
          >
            {LANGUAGE_OPTIONS.map((option) => (
              <option key={option.id} value={option.id}>
                {option.label}
              </option>
            ))}
          </select>
          <span className="gem-viewer-facts">
            {formatSize(size)}
            {info.dir && (
              <>
                <Separator />
                {info.dir}
              </>
            )}
          </span>
        </div>
      </footer>
    </div>
  );
}

type OutputProps = {
  connId: string;
  tab: FileTab;
  language: Extension | null;
  wrap: boolean;
  onEscape: () => void;
  onSynced: (state: { dropped: number; size: number }) => void;
};

/** The CodeView of one run, kept in step with its `CommandOutput`. */
function OutputView({ connId, tab, language, wrap, onEscape, onSynced }: OutputProps) {
  const code = useRef<CodeViewHandle>(null);
  const output = commandOutput(connId, tab.key);
  // The document starts from what has arrived so far; the rest streams in.
  const [initial] = useState(() => ({
    text: output.text,
    stderr: output.stderrBetween(output.start, output.end, output.start),
    start: output.start,
    end: output.end,
  }));
  const extensions = useMemo(() => stderrField(initial.stderr), [initial]);
  const synced = useRef({ start: initial.start, end: initial.end });
  const following = useRef(true);
  const [paused, setPaused] = useState(false);
  const latest = useRef(onSynced);
  useEffect(() => {
    latest.current = onSynced;
  });

  useEffect(() => {
    let frame: number | null = null;
    let timer: number | null = null;

    const flush = () => {
      if (frame !== null) cancelAnimationFrame(frame);
      if (timer !== null) window.clearTimeout(timer);
      frame = null;
      timer = null;
      const view = code.current?.view();
      if (!view) return;
      const doc = view.state.doc;
      const { start, end } = synced.current;
      if (output.start === start && output.end === end) return;
      const changes: { from: number; to?: number; insert?: string }[] = [];
      let ranges: Range[];
      if (end < output.start || output.end < end) {
        // Everything shown was dropped (or the output was cleared): replace it.
        changes.push({ from: 0, to: doc.length, insert: output.text });
        ranges = output.stderrBetween(output.start, output.end, output.start);
      } else {
        const removed = Math.min(output.start - start, doc.length);
        if (removed > 0) changes.push({ from: 0, to: removed });
        changes.push({ from: doc.length, insert: output.text.slice(end - output.start) });
        ranges = output.stderrBetween(end, output.end, output.start);
      }
      view.dispatch({ changes, effects: addStderr.of(ranges) });
      synced.current = { start: output.start, end: output.end };
      if (following.current) {
        view.dispatch({
          effects: EditorView.scrollIntoView(view.state.doc.length, { y: 'end' }),
        });
      }
      latest.current({ dropped: output.dropped, size: output.text.length });
    };

    const schedule = () => {
      if (frame !== null || timer !== null) return;
      // One update per frame; the timeout covers webviews that skip frames while hidden.
      if (typeof requestAnimationFrame === 'function') frame = requestAnimationFrame(flush);
      timer = window.setTimeout(flush, 100);
    };

    const view = code.current?.view();
    const onScroll = () => {
      const scroller = view?.scrollDOM;
      if (!scroller) return;
      const atEnd = scroller.scrollTop + scroller.clientHeight >= scroller.scrollHeight - 8;
      following.current = atEnd;
      setPaused(!atEnd);
    };
    view?.scrollDOM.addEventListener('scroll', onScroll, { passive: true });
    const stop = output.subscribe(schedule);
    schedule();
    return () => {
      stop();
      view?.scrollDOM.removeEventListener('scroll', onScroll);
      if (frame !== null) cancelAnimationFrame(frame);
      if (timer !== null) window.clearTimeout(timer);
    };
  }, [output]);

  return (
    <div className="gem-command-output">
      <CodeView
        ref={code}
        docKey={tab.key}
        initialText={initial.text}
        docVersion={0}
        language={language}
        readOnly
        wrap={wrap}
        label={t.outputLabel(tab.name)}
        extensions={extensions}
        onEscape={onEscape}
      />
      {paused && (
        <button
          type="button"
          className="mnml-button gem-log-resume"
          onClick={() => {
            following.current = true;
            setPaused(false);
            const view = code.current?.view();
            view?.dispatch({
              effects: EditorView.scrollIntoView(view.state.doc.length, { y: 'end' }),
            });
          }}
        >
          {t.paused}
        </button>
      )}
    </div>
  );
}
