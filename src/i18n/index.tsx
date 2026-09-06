import {
  createContext,
  useContext,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { en } from "./en";
import { zh } from "./zh";
import { de } from "./de";
import { fr } from "./fr";
import { ru } from "./ru";
import { ja } from "./ja";
import { ko } from "./ko";
import { es } from "./es";

export type Locale = "en" | "zh" | "de" | "fr" | "ru" | "ja" | "ko" | "es";
export type MessageKey = keyof typeof en;

/** English is the master dictionary; every other locale is partial and
 *  falls back to it key-by-key, so an untranslated string is English
 *  rather than a hole. Backend-generated text (skip reasons, errors)
 *  stays English by design - it is diagnostic and carries paths/ids. */
const DICTS: Record<Locale, Partial<Record<MessageKey, string>>> = {
  en,
  zh,
  de,
  fr,
  ru,
  ja,
  ko,
  es,
};

export const LOCALES: Locale[] = ["en", "zh", "de", "fr", "ru", "ja", "ko", "es"];

export const LOCALE_NAMES: Record<Locale, string> = {
  en: "English",
  zh: "中文",
  de: "Deutsch",
  fr: "Français",
  ru: "Русский",
  ja: "日本語",
  ko: "한국어",
  es: "Español",
};

function isLocale(v: string | null): v is Locale {
  return v !== null && (LOCALES as string[]).includes(v);
}

function detect(): Locale {
  const saved = localStorage.getItem("locale");
  if (isLocale(saved)) return saved;
  const nav = (navigator.language || "en").slice(0, 2).toLowerCase();
  return isLocale(nav) ? nav : "en";
}

function format(
  template: string,
  vars?: Record<string, string | number>
): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (m, k) =>
    vars[k] !== undefined ? String(vars[k]) : m
  );
}

export type T = (
  key: MessageKey,
  vars?: Record<string, string | number>
) => string;

interface I18n {
  locale: Locale;
  setLocale: (l: Locale) => void;
  t: T;
}

const Ctx = createContext<I18n | null>(null);

export function I18nProvider({ children }: { children: ReactNode }) {
  const [locale, setLocaleState] = useState<Locale>(detect);
  const value = useMemo<I18n>(
    () => ({
      locale,
      setLocale: (l: Locale) => {
        localStorage.setItem("locale", l);
        setLocaleState(l);
      },
      t: (key, vars) => format(DICTS[locale][key] ?? en[key], vars),
    }),
    [locale]
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18n {
  const c = useContext(Ctx);
  if (!c) throw new Error("useI18n used outside I18nProvider");
  return c;
}
