/**
 * Guards Gem's tokens like Piw's `tokens.test.ts`: every text colour Gem adds (syntax,
 * danger and accent text) reaches WCAG AA (4.5:1) on the canvas in both modes and with every
 * accent, and no raw colour appears outside `tokens.css`. `color-mix(in srgb, …)` is resolved
 * here against the kit's own token values.
 */
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';

const root = join(__dirname, '..', '..');
const kitTokens = readFileSync(
  join(root, 'node_modules', '@mnml-tools', 'ui', 'dist', 'styles', 'tokens.css'),
  'utf8',
);
const gemTokens = readFileSync(join(__dirname, 'tokens.css'), 'utf8');

type Tokens = Record<string, string>;

function declarations(block: string): Tokens {
  const tokens: Tokens = {};
  for (const match of block.matchAll(/(--[\w-]+):\s*([^;]+);/g))
    tokens[match[1]!] = match[2]!.trim();
  return tokens;
}

/** The body of the first rule whose selector is exactly `selector`. */
function block(css: string, selector: string): string {
  const start = css.indexOf(`${selector} {`);
  if (start < 0) throw new Error(`missing ${selector}`);
  return css.slice(start, css.indexOf('}', start));
}

const kitLight = declarations(block(kitTokens, ':root'));
const kitDark = { ...kitLight, ...declarations(block(kitTokens, ":root[data-theme='dark']")) };
const ACCENTS = ['pink', 'blue', 'orange', 'green', 'purple'] as const;
const accentValue = (accent: string) =>
  declarations(block(kitTokens, `:root[data-accent='${accent}'],\n  [data-swatch='${accent}']`))[
    '--accent'
  ]!;

const gemLight = declarations(block(gemTokens, ':root'));
const gemDark = { ...gemLight, ...declarations(block(gemTokens, ":root[data-theme='dark']")) };

type Rgb = [number, number, number];

function hex(value: string): Rgb {
  const clean = value.replace('#', '');
  return [0, 2, 4].map((offset) => parseInt(clean.slice(offset, offset + 2), 16)) as Rgb;
}

function resolve(value: string, tokens: Tokens, depth = 0): Rgb {
  if (depth > 10) throw new Error(`token loop at ${value}`);
  const trimmed = value.trim();
  if (trimmed.startsWith('#')) return hex(trimmed);
  const variable = /^var\((--[\w-]+)(?:,\s*(.+))?\)$/.exec(trimmed);
  if (variable) {
    const target = tokens[variable[1]!] ?? variable[2];
    if (!target) throw new Error(`unknown token ${variable[1]}`);
    return resolve(target, tokens, depth + 1);
  }
  const mix = /^color-mix\(in srgb,\s*(.+?)\s+(\d+)%,\s*(.+)\)$/.exec(trimmed);
  if (mix) {
    const a = resolve(mix[1]!, tokens, depth + 1);
    const b = resolve(mix[3]!, tokens, depth + 1);
    const p = Number(mix[2]) / 100;
    return a.map((channel, index) => Math.round(channel * p + b[index]! * (1 - p))) as Rgb;
  }
  throw new Error(`cannot resolve ${value}`);
}

function luminance([r, g, b]: Rgb): number {
  const linear = [r, g, b].map((channel) => {
    const value = channel / 255;
    return value <= 0.03928 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * linear[0]! + 0.7152 * linear[1]! + 0.0722 * linear[2]!;
}

function contrast(a: Rgb, b: Rgb): number {
  const [light, dark] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (light! + 0.05) / (dark! + 0.05);
}

const TEXT_TOKENS = Object.keys(gemLight).filter(
  (name) =>
    name.startsWith('--gem-syntax-') ||
    name === '--gem-danger-text' ||
    name === '--gem-accent-text',
);

describe('Gem tokens', () => {
  it('defines the syntax roles CodeView uses', () => {
    const used = readFileSync(
      join(__dirname, '..', 'components', 'ui', 'CodeView', 'theme.ts'),
      'utf8',
    );
    for (const match of used.matchAll(/syntax\('([\w-]+)'\)/g)) {
      expect(gemLight).toHaveProperty(`--gem-syntax-${match[1]}`);
    }
    expect(TEXT_TOKENS.length).toBeGreaterThan(12);
  });

  for (const [mode, kit, gem] of [
    ['light', kitLight, gemLight],
    ['dark', kitDark, gemDark],
  ] as const) {
    for (const accent of ACCENTS) {
      it(`keeps every text token at AA on the canvas (${mode}, ${accent})`, () => {
        const tokens: Tokens = { ...kit, ...gem, '--accent': accentValue(accent) };
        const canvas = resolve('var(--background)', tokens);
        const failures = TEXT_TOKENS.map((name) => ({
          name,
          ratio: contrast(resolve(`var(${name})`, tokens), canvas),
        })).filter((item) => item.ratio < 4.5);
        expect(failures).toEqual([]);
      });
    }
  }

  it('keeps raw colours inside tokens.css only', () => {
    const offenders: string[] = [];
    const walk = (dir: string) => {
      for (const name of readdirSync(dir)) {
        const path = join(dir, name);
        if (statSync(path).isDirectory()) {
          if (name !== 'dev' && name !== 'test') walk(path);
          continue;
        }
        if (!/\.(css|tsx?)$/.test(name) || name.endsWith('.test.ts') || name === 'tokens.css')
          continue;
        const text = readFileSync(path, 'utf8');
        if (
          /#[0-9a-f]{3,8}\b(?![\w-])/i.test(text.replace(/&#\d+;|#\w+-\w+/g, '')) &&
          /\.css$/.test(name)
        ) {
          offenders.push(path);
        }
        if (/\b(rgba?|hsla?)\(/.test(text)) offenders.push(path);
      }
    };
    walk(join(__dirname, '..'));
    expect(offenders).toEqual([]);
  });
});
