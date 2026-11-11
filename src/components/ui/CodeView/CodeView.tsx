/**
 * CodeView (`mnml-code-view`): a CodeMirror 6 editor with the family look (theme.ts), line
 * numbers, folding, indent guides, bracket matching, search (Mod+F), go to line (Mod+G),
 * soft wrap and a read-only mode, the last three switched through compartments.
 *
 * One view shows several documents: each `docKey` keeps its own EditorState (text, undo
 * history, selection, scroll), so switching file tabs is instant and loses nothing. A key's
 * document is (re)created from `initialText` the first time it is shown and whenever
 * `docVersion` changes (a reload). Dirty tracking compares the document with the last loaded
 * or saved text (`markSaved`).
 */
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import {
  bracketMatching,
  foldGutter,
  foldKeymap,
  indentOnInput,
  indentUnit,
  syntaxHighlighting,
} from '@codemirror/language';
import { gotoLine, highlightSelectionMatches, search, searchKeymap } from '@codemirror/search';
import {
  Annotation,
  Compartment,
  EditorState,
  type Extension,
  type StateEffect,
  type Text,
} from '@codemirror/state';
import {
  drawSelection,
  EditorView,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  keymap,
  lineNumbers,
  type ViewUpdate,
} from '@codemirror/view';
import { forwardRef, useEffect, useImperativeHandle, useLayoutEffect, useRef } from 'react';
import { guessIndent, indentGuides } from './indentation';
import { codeViewHighlight, codeViewTheme } from './theme';
import './CodeView.css';

export type CodeViewHandle = {
  focus: () => void;
  /** Current text of a document (the shown one by default); `null` when unknown. */
  getText: (key?: string) => string | null;
  /** Appends loaded text (not an edit: the document stays clean). */
  append: (key: string, text: string) => void;
  /** The document now matches the remote file. */
  markSaved: (key: string) => void;
  /** Drops a closed document's state. */
  forget: (key: string) => void;
  /** The underlying view, for callers that stream into the shown document. */
  view: () => EditorView | null;
};

type Props = {
  docKey: string;
  initialText: string;
  docVersion: number;
  /** Language support; `null` while it loads or for plain text. */
  language: Extension | null;
  readOnly: boolean;
  wrap: boolean;
  /** Accessible name of the text area, e.g. the file name. */
  label: string;
  onDirtyChange?: (key: string, dirty: boolean) => void;
  onCursor?: (line: number, column: number) => void;
  onEscape?: () => void;
  /** Extra extensions for each document's state (read when a document is created). */
  extensions?: Extension;
  className?: string;
};

type Entry = {
  state: EditorState;
  baseline: Text;
  version: number;
  dirty: boolean;
  /** Where the document was scrolled when another one replaced it. */
  scroll: StateEffect<unknown> | null;
};

/** Changes that come from the server (load more), not from the user. */
const remote = Annotation.define<boolean>();

export const CodeView = forwardRef<CodeViewHandle, Props>(function CodeView(props, ref) {
  const host = useRef<HTMLDivElement>(null);
  const view = useRef<EditorView | null>(null);
  const entries = useRef(new Map<string, Entry>());
  const currentKey = useRef<string | null>(null);
  const latest = useRef(props);
  latest.current = props;
  const compartments = useRef({
    language: new Compartment(),
    readOnly: new Compartment(),
    wrap: new Compartment(),
  });

  const configuration = () => {
    const { language, readOnly, wrap } = latest.current;
    const c = compartments.current;
    return [
      c.language.reconfigure(language ?? []),
      c.readOnly.reconfigure(EditorState.readOnly.of(readOnly)),
      c.wrap.reconfigure(wrap ? EditorView.lineWrapping : []),
    ];
  };

  const createState = (key: string, text: string): EditorState => {
    const { language, readOnly, wrap, label, extensions } = latest.current;
    const c = compartments.current;
    const indent = guessIndent(text);
    return EditorState.create({
      doc: text,
      extensions: [
        lineNumbers(),
        foldGutter({ openText: '▾', closedText: '▸' }),
        highlightActiveLineGutter(),
        highlightSpecialChars(),
        history(),
        drawSelection(),
        indentOnInput(),
        bracketMatching(),
        highlightActiveLine(),
        highlightSelectionMatches(),
        search({ top: true }),
        indentGuides(),
        indentUnit.of(indent.unit),
        EditorState.tabSize.of(indent.tabSize),
        syntaxHighlighting(codeViewHighlight),
        codeViewTheme,
        c.language.of(language ?? []),
        c.readOnly.of(EditorState.readOnly.of(readOnly)),
        c.wrap.of(wrap ? EditorView.lineWrapping : []),
        EditorView.contentAttributes.of({ 'aria-label': label }),
        keymap.of([
          {
            key: 'Escape',
            run: () => {
              latest.current.onEscape?.();
              return Boolean(latest.current.onEscape);
            },
          },
          { key: 'Mod-g', run: gotoLine, preventDefault: true },
          ...searchKeymap,
          ...foldKeymap,
          ...historyKeymap,
          indentWithTab,
          ...defaultKeymap,
        ]),
        EditorView.updateListener.of((update) => onUpdate(key, update)),
        extensions ?? [],
      ],
    });
  };

  const onUpdate = (key: string, update: ViewUpdate) => {
    const entry = entries.current.get(key);
    if (!entry) return;
    entry.state = update.state;
    if (update.docChanged) {
      if (update.transactions.some((tr) => tr.annotation(remote))) {
        entry.baseline = update.state.doc;
      }
      const dirty = !update.state.doc.eq(entry.baseline);
      if (dirty !== entry.dirty) {
        entry.dirty = dirty;
        latest.current.onDirtyChange?.(key, dirty);
      }
    }
    if ((update.selectionSet || update.docChanged) && key === currentKey.current) {
      const head = update.state.selection.main.head;
      const line = update.state.doc.lineAt(head);
      latest.current.onCursor?.(line.number, head - line.from + 1);
    }
  };

  /** Shows `key` in the view, creating or refreshing its state as needed. */
  const show = (key: string, version: number, text: string) => {
    const editor = view.current;
    if (!editor) return;
    const previous = currentKey.current;
    if (previous && entries.current.has(previous)) {
      const entry = entries.current.get(previous)!;
      entry.state = editor.state;
      entry.scroll = editor.scrollSnapshot();
    }
    let entry = entries.current.get(key);
    if (!entry || entry.version !== version) {
      const state = createState(key, text);
      const wasDirty = entry?.dirty ?? false;
      entry = { state, baseline: state.doc, version, dirty: false, scroll: null };
      entries.current.set(key, entry);
      if (wasDirty) latest.current.onDirtyChange?.(key, false);
    }
    currentKey.current = key;
    editor.setState(entry.state);
    editor.dispatch({
      effects: entry.scroll ? [...configuration(), entry.scroll] : configuration(),
    });
    const head = editor.state.selection.main.head;
    const line = editor.state.doc.lineAt(head);
    latest.current.onCursor?.(line.number, head - line.from + 1);
  };

  useLayoutEffect(() => {
    const parent = host.current!;
    const editor = new EditorView({ parent });
    view.current = editor;
    const { docKey, docVersion, initialText } = latest.current;
    show(docKey, docVersion, initialText);
    return () => {
      editor.destroy();
      view.current = null;
      currentKey.current = null;
    };
    // The view is created once; documents are swapped by the effect below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useLayoutEffect(() => {
    const entry = entries.current.get(props.docKey);
    if (currentKey.current === props.docKey && entry?.version === props.docVersion) return;
    show(props.docKey, props.docVersion, props.initialText);
    // `initialText` is only read when the key or version changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.docKey, props.docVersion]);

  useEffect(() => {
    view.current?.dispatch({ effects: configuration() });
  }, [props.language, props.readOnly, props.wrap]);

  useImperativeHandle(ref, () => ({
    focus: () => view.current?.focus(),
    getText: (key) => {
      const target = key ?? currentKey.current;
      if (!target) return null;
      if (target === currentKey.current && view.current) return view.current.state.doc.toString();
      return entries.current.get(target)?.state.doc.toString() ?? null;
    },
    append: (key, text) => {
      if (!text) return;
      if (key === currentKey.current && view.current) {
        const length = view.current.state.doc.length;
        view.current.dispatch({
          changes: { from: length, insert: text },
          annotations: remote.of(true),
        });
        return;
      }
      const entry = entries.current.get(key);
      if (!entry) return;
      const state = entry.state.update({
        changes: { from: entry.state.doc.length, insert: text },
        annotations: remote.of(true),
      }).state;
      entry.state = state;
      entry.baseline = state.doc;
    },
    markSaved: (key) => {
      const entry = entries.current.get(key);
      if (!entry) return;
      if (key === currentKey.current && view.current) entry.state = view.current.state;
      entry.baseline = entry.state.doc;
      if (entry.dirty) {
        entry.dirty = false;
        latest.current.onDirtyChange?.(key, false);
      }
    },
    view: () => view.current,
    forget: (key) => {
      if (key !== currentKey.current) entries.current.delete(key);
    },
  }));

  return (
    <div
      ref={host}
      className={props.className ? `mnml-code-view ${props.className}` : 'mnml-code-view'}
    />
  );
});
