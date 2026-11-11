/**
 * Indentation helpers for CodeView: the indent unit guessed from the document, and indent
 * guides drawn as a background on each line's leading whitespace (visible lines only).
 */
import { getIndentUnit, indentUnit } from '@codemirror/language';
import { RangeSetBuilder, type Extension } from '@codemirror/state';
import {
  Decoration,
  EditorView,
  ViewPlugin,
  type DecorationSet,
  type ViewUpdate,
} from '@codemirror/view';

/** Tabs, or the most common step between indented lines (2, 3, 4 or 8 spaces). */
export function guessIndent(text: string): { unit: string; tabSize: number } {
  let tabs = 0;
  let spaced = 0;
  const steps = new Map<number, number>();
  let previous = 0;
  let lines = 0;
  for (const match of text.slice(0, 200_000).matchAll(/^([ \t]*)\S/gm)) {
    lines += 1;
    if (lines > 2000) break;
    const indent = match[1] ?? '';
    if (indent.startsWith('\t')) {
      tabs += 1;
      continue;
    }
    const width = indent.length;
    if (width > 0) spaced += 1;
    const step = Math.abs(width - previous);
    if (step >= 2 && step <= 8) steps.set(step, (steps.get(step) ?? 0) + 1);
    previous = width;
  }
  if (tabs > spaced) return { unit: '\t', tabSize: 4 };
  let best = 4;
  let bestCount = 0;
  for (const [step, count] of steps) {
    if (count > bestCount || (count === bestCount && step < best)) {
      best = step;
      bestCount = count;
    }
  }
  return { unit: ' '.repeat(best), tabSize: best };
}

const guide = Decoration.mark({ class: 'cm-indent-guides' });

function buildGuides(view: EditorView): DecorationSet {
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    let position = from;
    while (position <= to) {
      const line = view.state.doc.lineAt(position);
      const leading = /^[ \t]*/.exec(line.text)?.[0].length ?? 0;
      // Whitespace-only lines get no guide (they would end mid-air).
      if (leading > 0 && leading < line.length) builder.add(line.from, line.from + leading, guide);
      position = line.to + 1;
    }
  }
  return builder.finish();
}

const guidesPlugin = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildGuides(view);
    }
    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged) this.decorations = buildGuides(update.view);
    }
  },
  { decorations: (plugin) => plugin.decorations },
);

/** Indent guides spaced by the state's indent unit (`--gem-indent-unit`, in characters). */
export function indentGuides(): Extension {
  return [
    guidesPlugin,
    EditorView.editorAttributes.compute([indentUnit], (state) => ({
      style: `--gem-indent-unit: ${getIndentUnit(state)}`,
    })),
  ];
}
