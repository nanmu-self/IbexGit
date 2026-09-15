/**
 * i18n layer (P2). Reactive via Svelte 5 runes: `t()` reads `i18n.locale`
 * (a `$state` object), so every template that renders `t(...)` re-renders
 * when the locale switches. See ADR-011 for why this replaces svelte-i18n.
 */
import en from "./dictionaries/en.json";
import zhCN from "./dictionaries/zh-CN.json";

export type Locale = "zh-CN" | "en";

type Dict = Record<string, string>;

const dictionaries: Record<Locale, Dict> = {
  "zh-CN": zhCN as Dict,
  en: en as Dict,
};

/** Fallback chain: current locale → zh-CN → raw key. */
const FALLBACK: Locale = "zh-CN";

export const i18n = $state<{ locale: Locale }>({ locale: FALLBACK });

export type I18nParams = Record<string, string | number>;

export function t(key: string, params?: I18nParams): string {
  const dict = dictionaries[i18n.locale] ?? dictionaries[FALLBACK];
  let text = dict[key] ?? dictionaries[FALLBACK][key] ?? key;
  if (params) {
    for (const [name, value] of Object.entries(params)) {
      text = text.replaceAll(`{${name}}`, String(value));
    }
  }
  return text;
}

export function setLocale(locale: Locale): void {
  i18n.locale = locale;
}
