/**
 * Gem's own stream parsers for the configuration formats that live on servers, where the
 * legacy modes are too coarse (nginx colours whole phrases as numbers, `properties` leaves
 * values plain). Token names are Lezer tag names (`variableName.special` = special(variableName)),
 * so they take the roles of `CodeView/theme.ts` like every other language.
 */
import type { StreamParser } from '@codemirror/language';

const NUMBER = /^-?\d[\d_]*(?:\.\d+)*(?:[eE][+-]?\d+)?(?:[kKmMgGtT][bB]?|ms|[smhdwy]|%)?(?![\w.-])/;
const BOOLEAN = /^(?:true|false|yes|no|on|off|null|none|nil)(?![\w.-])/i;

/** Quoted strings with `\` escapes; `''` and `""`, unterminated up to the end of the line. */
function quoted(stream: Parameters<StreamParser<unknown>['token']>[0]): boolean {
  return !!(stream.match(/^"(?:[^"\\]|\\.)*"?/) || stream.match(/^'(?:[^'\\]|\\.)*'?/));
}

type ConfState = { inValue: boolean };

/**
 * `.env`, INI and Java properties: `[sections]`, `export`, keys, `=`/`:`, and values that are
 * strings, numbers, booleans or `${VARIABLES}`.
 */
export const conf: StreamParser<ConfState> = {
  name: 'conf',
  startState: () => ({ inValue: false }),
  token(stream, state) {
    if (stream.sol()) state.inValue = false;
    if (stream.eatSpace()) return null;
    if (!state.inValue) {
      if (stream.sol() || /^\s*$/.test(stream.string.slice(0, stream.start))) {
        if (stream.match(/^[#;!].*/)) return 'comment';
        if (stream.match(/^\[[^\]]*\]?/)) return 'heading';
        if (stream.match(/^export(?=\s)/)) return 'keyword';
      }
      if (stream.match(/^[^=:\s#;]+(?=\s*[=:])/)) return 'propertyName';
      if (stream.match(/^[=:]/)) {
        state.inValue = true;
        return 'operator';
      }
      stream.skipToEnd();
      return 'propertyName';
    }
    if (stream.match(/^#.*/) && /\s$/.test(stream.string.slice(0, stream.start))) return 'comment';
    if (quoted(stream)) return 'string';
    if (stream.match(/^\$\{[^}]*\}?/) || stream.match(/^\$\w+/)) return 'variableName.special';
    if (stream.match(NUMBER)) return 'number';
    if (stream.match(BOOLEAN)) return 'bool';
    if (stream.match(/^[^\s$"'#]+/)) return 'string';
    stream.next();
    return 'string';
  },
  languageData: { commentTokens: { line: '#' } },
};

type NginxState = { statementStart: boolean; regexNext: boolean };

/** nginx: directives, block names, `$variables`, `~` regex locations, sizes and times. */
export const nginx: StreamParser<NginxState> = {
  name: 'nginx',
  startState: () => ({ statementStart: true, regexNext: false }),
  token(stream, state) {
    if (stream.eatSpace()) return null;
    if (stream.match(/^#.*/)) return 'comment';
    if (stream.eat(';')) {
      state.statementStart = true;
      return 'punctuation';
    }
    if (stream.eat('{') || stream.eat('}')) {
      state.statementStart = true;
      state.regexNext = false;
      return 'brace';
    }
    if (state.statementStart && stream.match(/^[A-Za-z_][\w.-]*/)) {
      state.statementStart = false;
      return 'keyword';
    }
    if (stream.match(/^(?:~\*|\^~|~|=)(?=\s)/)) {
      state.regexNext = stream.current() !== '=';
      return 'operator';
    }
    if (state.regexNext && stream.match(/^\S+/)) {
      state.regexNext = false;
      return 'regexp';
    }
    if (quoted(stream)) return 'string';
    if (stream.match(/^\$\{?\w+\}?/)) return 'variableName.special';
    if (stream.match(/^(?:on|off)(?![\w.-])/)) return 'atom';
    if (stream.match(/^\d{1,3}(?:\.\d{1,3}){3}(?::\d+)?(?:\/\d+)?(?![\w.])/)) return 'number';
    if (stream.match(/^\[?[0-9a-f:]*:[0-9a-f:]*\]?(?::\d+)?(?=[\s;])/i)) return 'number';
    if (stream.match(NUMBER)) return 'number';
    if (stream.match(/^(?:https?|unix|fastcgi):\/?\/?[^\s;{}]*/)) return 'url';
    if (stream.match(/^\/[^\s;{}]*/)) return 'string';
    if (stream.match(/^[^\s;{}"'$]+/)) return null;
    stream.next();
    return null;
  },
  languageData: { commentTokens: { line: '#' } },
};

const INSTRUCTIONS =
  /^(?:FROM|RUN|CMD|LABEL|MAINTAINER|EXPOSE|ENV|ADD|COPY|ENTRYPOINT|VOLUME|USER|WORKDIR|ARG|ONBUILD|STOPSIGNAL|HEALTHCHECK|SHELL)(?=\s|$)/i;

type DockerState = { instruction: string | null; continued: boolean };

/** Dockerfile: instructions, `--flags=`, `KEY=value`, `$VARIABLES`, strings, `AS name`. */
export const dockerfile: StreamParser<DockerState> = {
  name: 'dockerfile',
  startState: () => ({ instruction: null, continued: false }),
  token(stream, state) {
    if (stream.sol()) {
      if (!state.continued) state.instruction = null;
      state.continued = /\\\s*$/.test(stream.string);
    }
    if (stream.eatSpace()) return null;
    if (state.instruction === null) {
      if (stream.match(/^#.*/)) return 'comment';
      if (stream.match(INSTRUCTIONS)) {
        state.instruction = stream.current().toUpperCase();
        return 'keyword';
      }
    }
    if (stream.match(/^#.*/) && stream.start === stream.string.search(/\S/)) return 'comment';
    if (state.instruction === 'FROM' && stream.match(/^AS(?=\s)/i)) return 'keyword';
    if (stream.match(/^--[\w-]+(?==)/)) return 'propertyName';
    if (stream.match(/^--?[A-Za-z][\w-]*/)) return 'meta';
    if (
      (state.instruction === 'ENV' ||
        state.instruction === 'ARG' ||
        state.instruction === 'LABEL') &&
      stream.match(/^[\w.-]+(?==)/)
    ) {
      return 'propertyName';
    }
    if (quoted(stream)) return 'string';
    if (stream.match(/^\$\{[^}]*\}?/) || stream.match(/^\$\w+/)) return 'variableName.special';
    if (stream.match(/^(?:&&|\|\||[|=;\\[\],])/)) return 'operator';
    if (stream.match(NUMBER)) return 'number';
    if (stream.match(/^[^\s"'$=&|;\\[\],]+/)) return null;
    stream.next();
    return null;
  },
  languageData: { commentTokens: { line: '#' } },
};

/** Apache httpd configuration: directives, `<Section>` tags, flags, variables. */
export const apache: StreamParser<{ directive: boolean }> = {
  name: 'apache',
  startState: () => ({ directive: true }),
  token(stream, state) {
    if (stream.sol()) state.directive = true;
    if (stream.eatSpace()) return null;
    if (stream.match(/^#.*/)) return 'comment';
    if (stream.match(/^<\/?[A-Za-z][\w-]*/)) {
      state.directive = false;
      return 'tagName';
    }
    if (stream.eat('>')) return 'tagName';
    if (quoted(stream)) return 'string';
    if (stream.match(/^[%$]\{[^}]*\}?/)) return 'variableName.special';
    if (stream.match(/^\[[A-Z0-9,=_ ]+\]/i)) return 'meta';
    if (state.directive && stream.match(/^[A-Za-z][\w-]*/)) {
      state.directive = false;
      return 'keyword';
    }
    if (stream.match(/^(on|off)\b/i)) return 'atom';
    if (stream.match(/^\d+(\.\d+)*\b/)) return 'number';
    if (stream.match(/^\^\S*/)) return 'regexp';
    if (stream.match(/^\/[^\s"'>]*/)) return 'string';
    if (stream.match(/^[^\s"'<>%$]+/)) return null;
    stream.next();
    return null;
  },
  languageData: { commentTokens: { line: '#' } },
};

/** Application and server logs: timestamps, levels, quoted strings, numbers. */
export const log: StreamParser<null> = {
  name: 'log',
  startState: () => null,
  token(stream) {
    if (stream.eatSpace()) return null;
    if (
      stream.match(
        /^\[?\d{4}-\d{2}-\d{2}[T ]\d{2}:\d{2}:\d{2}(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?\]?/,
      )
    ) {
      return 'meta';
    }
    if (stream.match(/^\[\d{2}\/\w{3}\/\d{4}:\d{2}:\d{2}:\d{2} [+-]\d{4}\]/)) return 'meta';
    if (stream.match(/^(?:\w+\.)?(?:ERROR|FATAL|CRITICAL|CRIT|EMERGENCY|EMERG|ALERT|SEVERE)\b:?/)) {
      return 'invalid';
    }
    if (stream.match(/^(?:\w+\.)?(?:WARN|WARNING)\b:?/)) return 'atom';
    if (stream.match(/^(?:\w+\.)?(?:INFO|NOTICE)\b:?/)) return 'keyword';
    if (stream.match(/^(?:\w+\.)?(?:DEBUG|TRACE)\b:?/)) return 'comment';
    if (stream.match(/^\[[\w.:-]+\]/)) return 'typeName';
    if (stream.match(/^"(?:[^"\\]|\\.)*"?/)) return 'string';
    if (stream.match(/^(?:GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b/))
      return 'variableName.function';
    if (stream.match(/^\d+(?:\.\d+)*(?:ms|s|B|KB|MB)?\b/)) return 'number';
    if (stream.match(/^[\w-]+(?==)/)) return 'propertyName';
    if (stream.match(/^\/[^\s"]*/)) return 'string';
    if (stream.match(/^[^\s"[/]+/)) return null;
    stream.next();
    return null;
  },
};

const SHELL_KEYWORDS = new Set(['readonly', 'local', 'declare', 'typeset', 'function', 'select']);
const SHELL_BUILTINS = new Set([
  'printf',
  'read',
  'trap',
  'source',
  'exec',
  'eval',
  'shift',
  'wait',
  'command',
  'getopts',
  'umask',
  'ulimit',
]);

/**
 * The legacy shell mode, with its token names refined for Gem's roles: assignments and `$VAR`
 * (`def`) are variables, known commands (`builtin`) functions, `-flags` (`property`) quiet meta.
 */
export function refineShell<S>(shell: StreamParser<S>): StreamParser<S> {
  const names: Record<string, string> = {
    def: 'variableName.special',
    builtin: 'variableName.function',
    property: 'meta',
    attribute: 'meta',
  };
  return {
    ...shell,
    name: shell.name,
    token(stream, state) {
      const token = shell.token(stream, state);
      if (token) return names[token] ?? token;
      // Words the legacy mode leaves plain: declaration keywords and common builtins.
      const word = stream.current();
      if (SHELL_KEYWORDS.has(word)) return 'keyword';
      if (SHELL_BUILTINS.has(word)) return 'variableName.function';
      return token;
    },
  };
}

/** The legacy TOML mode with `[tables]` as headings (it returns them as atoms, like `true`). */
export function refineToml<S>(toml: StreamParser<S>): StreamParser<S> {
  return {
    ...toml,
    token(stream, state) {
      const token = toml.token(stream, state);
      return token && stream.current().startsWith('[') && /\]$/.test(stream.current())
        ? 'heading'
        : token;
    },
  };
}
