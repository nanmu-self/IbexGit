/**
 * i18n layer (P2). Reactive via Svelte 5 runes: `t()` reads `i18n.locale`
 * (a `$state` object), so every template that renders `t(...)` re-renders
 * when the locale switches. See ADR-011 for why this replaces svelte-i18n.
 *
 * Dictionaries load lazily per locale (dynamic import): only the active
 * locale ships in the startup bundle — the other becomes its own lazy
 * chunk fetched only when the user switches languages (~13 KB gz saved
 * at startup). `setLocale` awaits the dictionary before flipping the
 * `$state`, so `t()` never renders a raw key mid-switch. The load window
 * is covered by the startup spinner gated on `settings.ready`.
 */
export type Locale = "zh-CN" | "en";

type Dict = Record<string, string>;

/** Locale → dictionary loader. Each JSON builds as its own lazy chunk. */
const loaders: Record<Locale, () => Promise<Dict>> = {
  "zh-CN": () =>
    import("./dictionaries/zh-CN.json").then((m) => m.default as Dict),
  en: () => import("./dictionaries/en.json").then((m) => m.default as Dict),
};

/** Loaded dictionaries by locale (populated on demand). */
const cache: Partial<Record<Locale, Dict>> = {};

/** Fallback chain: current locale → zh-CN (when loaded) → raw key.
 *  The two dictionaries have an identical key set (enforced by the
 *  `dict_parity` integration test in src-tauri/tests), so the mid-chain
 *  fallback is theoretical; the raw key is a last resort. */
export const FALLBACK_LOCALE: Locale = "zh-CN";
const FALLBACK: Locale = FALLBACK_LOCALE;

export const i18n = $state<{ locale: Locale }>({ locale: FALLBACK });

export type I18nParams = Record<string, string | number>;

/** Fetch + cache a dictionary (idempotent, deduped via the cache). */
function loadDict(locale: Locale): Promise<Dict> {
  const cached = cache[locale];
  if (cached) return Promise.resolve(cached);
  return loaders[locale]().then((dict) => {
    cache[locale] = dict;
    return dict;
  });
}

export function t(key: string, params?: I18nParams): string {
  let text = cache[i18n.locale]?.[key] ?? cache[FALLBACK]?.[key] ?? key;
  if (params) {
    for (const [name, value] of Object.entries(params)) {
      text = text.replaceAll(`{${name}}`, String(value));
    }
  }
  return text;
}

/**
 * Switch locale: fetch the dictionary first, then flip the reactive
 * state — callers awaiting this see the new language applied atomically.
 * Resolves `false` when the dictionary could not be loaded: the previous
 * locale stays in effect and the caller must not persist the new one.
 * Failures are logged (console.error is intercepted into the file log).
 */
export async function setLocale(locale: Locale): Promise<boolean> {
  try {
    await loadDict(locale);
  } catch (err) {
    console.error(`[i18n] failed to load dictionary for ${locale}`, err);
    return false;
  }
  i18n.locale = locale;
  return true;
}
