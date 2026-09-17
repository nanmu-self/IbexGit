/**
 * Syntax highlighting for the diff viewer (PLAN P4, ADR-010):
 * - `shiki/core` + JS regex engine (no WASM, ~1MB saved)
 * - grammars loaded per language via static import-map entries (Vite
 *   code-splits each grammar into its own lazy chunk; never in the initial
 *   bundle — a variable-path `import()` would not be analyzed at all and
 *   would 404 in the packaged build)
 * - highlighter singleton; per-hunk token cache with an LRU cap
 * - unsupported languages degrade to plain text (null tokens)
 */
import type { HighlighterCore, LanguageRegistration } from "shiki/core";

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

/**
 * Grammar id → static import. Vite cannot analyze a variable-path `import()`
 * (the grammar would be missing from the packaged build), so every grammar is
 * referenced statically here; Vite still code-splits each into its own lazy
 * chunk. Keys must cover every id `langIdFor` can return.
 */
const LANG_IMPORTS: Record<
  string,
  () => Promise<{ default: LanguageRegistration[] }>
> = {
  astro: () => import("shiki/langs/astro.mjs"),
  c: () => import("shiki/langs/c.mjs"),
  cpp: () => import("shiki/langs/cpp.mjs"),
  csharp: () => import("shiki/langs/csharp.mjs"),
  css: () => import("shiki/langs/css.mjs"),
  dart: () => import("shiki/langs/dart.mjs"),
  dockerfile: () => import("shiki/langs/dockerfile.mjs"),
  go: () => import("shiki/langs/go.mjs"),
  html: () => import("shiki/langs/html.mjs"),
  ini: () => import("shiki/langs/ini.mjs"),
  java: () => import("shiki/langs/java.mjs"),
  javascript: () => import("shiki/langs/javascript.mjs"),
  json: () => import("shiki/langs/json.mjs"),
  json5: () => import("shiki/langs/json5.mjs"),
  jsonc: () => import("shiki/langs/jsonc.mjs"),
  jsx: () => import("shiki/langs/jsx.mjs"),
  kotlin: () => import("shiki/langs/kotlin.mjs"),
  less: () => import("shiki/langs/less.mjs"),
  lua: () => import("shiki/langs/lua.mjs"),
  makefile: () => import("shiki/langs/makefile.mjs"),
  markdown: () => import("shiki/langs/markdown.mjs"),
  perl: () => import("shiki/langs/perl.mjs"),
  php: () => import("shiki/langs/php.mjs"),
  powershell: () => import("shiki/langs/powershell.mjs"),
  python: () => import("shiki/langs/python.mjs"),
  ruby: () => import("shiki/langs/ruby.mjs"),
  rust: () => import("shiki/langs/rust.mjs"),
  sass: () => import("shiki/langs/sass.mjs"),
  scala: () => import("shiki/langs/scala.mjs"),
  scss: () => import("shiki/langs/scss.mjs"),
  shellscript: () => import("shiki/langs/shellscript.mjs"),
  sql: () => import("shiki/langs/sql.mjs"),
  svelte: () => import("shiki/langs/svelte.mjs"),
  swift: () => import("shiki/langs/swift.mjs"),
  toml: () => import("shiki/langs/toml.mjs"),
  tsx: () => import("shiki/langs/tsx.mjs"),
  typescript: () => import("shiki/langs/typescript.mjs"),
  vue: () => import("shiki/langs/vue.mjs"),
  xml: () => import("shiki/langs/xml.mjs"),
  yaml: () => import("shiki/langs/yaml.mjs"),
};

const loadedLangs = new Set<string>();

async function loadLang(lang: string): Promise<boolean> {
  const hl = await getHighlighter();
  if (loadedLangs.has(lang)) return true;
  const loader = LANG_IMPORTS[lang];
  if (!loader) return false;
  try {
    await hl.loadLanguage(loader());
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
