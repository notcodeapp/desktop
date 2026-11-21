import * as monaco from 'monaco-editor';

export type ThemeName = 'dark' | 'light';
const STORAGE_KEY = 'notcode.theme';
export const THEME_EVENT = 'notcode:theme-changed';

export function currentTheme(): ThemeName {
  return window.localStorage.getItem(STORAGE_KEY) === 'light' ? 'light' : 'dark';
}

/// Reads a color token from the generated CSS custom properties so the editor
/// and terminal always follow the active design tokens.
function cssColor(variable: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(variable).trim();
}

interface Palette { background: string; sidebar: string; surface: string; divider: string; text: string; muted: string; accent: string }

function palette(): Palette {
  const v = (name: string) => cssColor(`--colors-${name}`) || '#000000';
  return {
    background: v('background'),
    sidebar: v('sidebar'),
    surface: v('surface'),
    divider: v('divider'),
    text: v('text'),
    muted: v('muted'),
    accent: v('accent'),
  };
}

function hex(c: string): string {
  return c.replace('#', '');
}

function monacoTheme(): monaco.editor.IStandaloneThemeData {
  const c = palette();
  return {
    base: currentTheme() === 'light' ? 'vs' : 'vs-dark',
    inherit: true,
    rules: [
      { token: 'comment', foreground: hex(c.muted), fontStyle: 'italic' },
      { token: 'keyword', foreground: hex(c.accent) },
      { token: 'string', foreground: hex(c.text) },
      { token: 'number', foreground: hex(c.accent) },
    ],
    colors: {
      'editor.background': c.background,
      'editor.foreground': c.text,
      'editorLineNumber.foreground': c.muted,
      'editorLineNumber.activeForeground': c.accent,
      'editorCursor.foreground': c.accent,
      'editor.selectionBackground': `${c.accent}24`,
      'editor.lineHighlightBackground': c.surface,
      'editorWidget.background': c.surface,
      'editorWidget.border': c.divider,
      'editorIndentGuide.background1': c.divider,
      focusBorder: c.accent,
    },
  };
}

export function xtermTheme(): Record<string, string> {
  const c = palette();
  return {
    background: c.sidebar,
    foreground: c.text,
    cursor: c.accent,
    selectionBackground: `${c.accent}30`,
    black: c.sidebar,
    red: cssColor('--colors-error') || c.text,
    green: cssColor('--colors-success') || c.text,
    yellow: cssColor('--colors-warning') || c.text,
    blue: c.muted,
    magenta: c.text,
    cyan: c.accent,
    white: c.text,
    brightBlack: c.divider,
    brightRed: cssColor('--colors-error') || c.text,
    brightGreen: cssColor('--colors-success') || c.text,
    brightYellow: cssColor('--colors-warning') || c.text,
    brightBlue: c.muted,
    brightMagenta: c.text,
    brightCyan: c.accent,
    brightWhite: c.text,
  };
}

/// Applies the theme to the document and re-defines the Monaco theme. Open
/// terminals listen for THEME_EVENT and repaint themselves.
export function applyTheme(name: ThemeName): void {
  window.localStorage.setItem(STORAGE_KEY, name);
  document.documentElement.dataset.theme = name;
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', palette().background);
  monaco.editor.defineTheme('notcode', monacoTheme());
  window.dispatchEvent(new CustomEvent(THEME_EVENT, { detail: name }));
}

/// Sets the initial theme before first paint of themed views.
export function initTheme(): void {
  document.documentElement.dataset.theme = currentTheme();
  monaco.editor.defineTheme('notcode', monacoTheme());
}
