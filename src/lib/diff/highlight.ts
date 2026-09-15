/**
 * Syntax highlighting for the diff viewer (PLAN P4, ADR-010):
 * - `shiki/core` + JS regex engine (no WASM, ~1MB saved)
 * - grammars loaded per language via dynamic import (Vite code-splits each
 *   grammar into its own chunk; never in the initial bundle)
 * - highlighter singleton; per-hunk token cache with an LRU cap
 * - unsupported languages degrade to plain text (null tokens)
 */
import type { HighlighterCore } from "shiki/core";

export type ShikiTheme = "github-light" | "github-dark";

let highlighterPromise: Promise<HighlighterCore> | null = null;

function getHighlighter(): Promise<HighlighterCore> {
  if (!highlighterPromise) {
    // Everything is a dynamic import (ADR-010): shiki core, the JS regex
    // engine and the two themes only load when the diff viewer first needs
    // to highlight — never in the initial bundle.
    highlighterPromise = Promise.all([
      import("shiki/core"),
      import("shiki/engine/javascript"),
      import("shiki/themes/github-light.mjs"),
      import("shiki/themes/github-dark.mjs"),
    ])
      .then(([{ createHighlighterCore }, { createJavaScriptRegexEngine }, light, dark]) =>
        createHighlighterCore({
          themes: [light, dark],
          langs: [],
          engine: createJavaScriptRegexEngine(),
        }),
      )
      .catch((e) => {
        highlighterPromise = null; // allow retry on next interaction
        throw e;
      });
  }
  return highlighterPromise;
}

/** Extension → Shiki grammar id for the bundled language set (~30). */
const LANG_BY_EXT: Record<string, string> = {
  ts: "typescript",
  mts: "typescript",
  cts: "typescript",
  tsx: "tsx",
  jsx: "jsx",
  js: "javascript",
  mjs: "javascript",
  cjs: "javascript",
  json: "json",
  jsonc: "jsonc",
  json5: "json5",
  css: "css",
  scss: "scss",
  sass: "sass",
  less: "less",
  html: "html",
  htm: "html",
  vue: "vue",
  svelte: "svelte",
  astro: "astro",
  py: "python",
  pyi: "python",
  rb: "ruby",
  go: "go",
  rs: "rust",
  java: "java",
  kt: "kotlin",
  kts: "kotlin",
  swift: "swift",
  c: "c",
  h: "c",
  cpp: "cpp",
  cc: "cpp",
  cxx: "cpp",
  hpp: "cpp",
  cs: "csharp",
  php: "php",
  sh: "shellscript",
  bash: "shellscript",
  zsh: "shellscript",
  ps1: "powershell",
  yaml: "yaml",
  yml: "yaml",
  toml: "toml",
  md: "markdown",
  markdown: "markdown",
  sql: "sql",
  xml: "xml",
  svg: "xml",
  lua: "lua",
  perl: "perl",
  pl: "perl",
  ini: "ini",
  conf: "ini",
  dart: "dart",
  scala: "scala",
  dockerfile: "dockerfile",
};

export function langIdFor(path: string | null | undefined): string | null {
  if (!path) return null;
  const base = path.split(/[\\/]/).pop() ?? path;
  // Full-name matches first (Dockerfile, Makefile…).
  const lower = base.toLowerCase();
  if (lower === "dockerfile") return "dockerfile";
  if (lower === "makefile") return "makefile";
  const ext = lower.includes(".") ? lower.split(".").pop()! : "";
  return LANG_BY_EXT[ext] ?? null;
}

const loadedLangs = new Set<string>();

async function loadLang(lang: string): Promise<boolean> {
  const hl = await getHighlighter();
  if (loadedLangs.has(lang)) return true;
  try {
    await hl.loadLanguage(import(`shiki/langs/${lang}.mjs`));
    loadedLangs.add(lang);
    return true;
  } catch {
    return false;
  }
}

export interface HunkHighlight {
  /** One token array per line, aligned with the hunk text lines. */
  lines: ThemedTokenLike[][];
}

/** Minimal token shape used by the renderer (avoids deep shiki imports). */
export interface ThemedTokenLike {
  content: string;
  color?: string;
  /** shiki font-style bits: 1=italic, 2=bold, 4=underline. */
  fontStyle?: number;
}

/** Tokens per line for a code snippet; null when highlighting is skipped. */
export async function highlightLines(
  textLines: string[],
  lang: string,
  theme: ShikiTheme,
): Promise<ThemedTokenLike[][] | null> {
  if (!(await loadLang(lang))) return null;
  const hl = await getHighlighter();
  try {
    const result = hl.codeToTokens(textLines.join("\n"), {
      lang,
      theme,
    });
    return result.tokens;
  } catch {
    return null;
  }
}

// ---------------- per-hunk cache ----------------

interface CacheEntry {
  tokens: ThemedTokenLike[][] | null;
  promise: Promise<ThemedTokenLike[][] | null>;
}

const cache = new Map<string, CacheEntry>();
const CACHE_MAX = 60;

function cacheKey(lines: string[], lang: string, theme: ShikiTheme): string {
  // Cheap content hash: length + first/last line is enough for cache hits on
  // unchanged hunks; model identity changes create new keys via callers.
  const head = lines[0] ?? "";
  const tail = lines[lines.length - 1] ?? "";
  return `${lang}|${theme}|${lines.length}|${head.length}:${head}|${tail}`;
}

function evictIfNeeded(): void {
  if (cache.size <= CACHE_MAX) return;
  const it = cache.keys();
  while (cache.size > CACHE_MAX) {
    const k = it.next();
    if (k.done) break;
    cache.delete(k.value);
  }
}

/** Get (or start) highlighting for one hunk body. */
export function getHunkTokens(
  lines: string[],
  lang: string,
  theme: ShikiTheme,
): Promise<ThemedTokenLike[][] | null> {
  const key = cacheKey(lines, lang, theme);
  const hit = cache.get(key);
  if (hit) return hit.promise;
  const entry: CacheEntry = {
    tokens: null,
    promise: highlightLines(lines, lang, theme).finally(() => evictIfNeeded()),
  };
  cache.set(key, entry);
  return entry.promise;
}

export function clearHighlightCache(): void {
  cache.clear();
}
