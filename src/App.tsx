import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ask } from "@tauri-apps/plugin-dialog";
import { check as checkUpdaterFeed } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { api } from "./api";
import type {
  AccountEntry,
  CharacterEntry,
  CharactersResponse,
  CommitEntry,
  Group,
  InstallSummary,
  MirrorStatus,
  Template,
  UpdateCheck,
} from "./types";
import { AccountsView } from "./components/AccountsView";
import { CharactersView } from "./components/CharactersView";
import { GroupsView } from "./components/GroupsView";
import { TemplatesView } from "./components/TemplatesView";
import { GitView } from "./components/GitView";
import { SettingsView } from "./components/SettingsView";
import { AboutView } from "./components/AboutView";
import { Banner } from "./components/Banner";
import { useI18n, type MessageKey } from "./i18n";

export type Tab =
  | "characters"
  | "accounts"
  | "groups"
  | "templates"
  | "git"
  | "settings"
  | "about";

const TABS: { key: Tab; label: MessageKey }[] = [
  { key: "characters", label: "dept_characters" },
  { key: "accounts", label: "dept_accounts" },
  { key: "groups", label: "dept_groups" },
  { key: "templates", label: "dept_templates" },
  { key: "git", label: "dept_git" },
  { key: "settings", label: "dept_settings" },
];

export default function App() {
  const { t } = useI18n();
  const [tab, setTab] = useState<Tab>("characters");

  // The tabs stay mounted and share one scroll container, so without
  // this a deep scroll in one tab bleeds into the next (clamped to
  // whatever height it has). Remember each tab's offset and put it
  // back before paint.
  const pageRef = useRef<HTMLElement | null>(null);
  const tabScroll = useRef<Partial<Record<Tab, number>>>({});
  const switchTab = (next: Tab) => {
    if (next === tab) return;
    if (pageRef.current) tabScroll.current[tab] = pageRef.current.scrollTop;
    setTab(next);
  };
  useLayoutEffect(() => {
    if (pageRef.current) pageRef.current.scrollTop = tabScroll.current[tab] ?? 0;
  }, [tab]);
  const [installs, setInstalls] = useState<InstallSummary | null>(null);
  const [chars, setChars] = useState<CharactersResponse | null>(null);
  const [accounts, setAccounts] = useState<AccountEntry[]>([]);
  const [groups, setGroups] = useState<Group[]>([]);
  const [templates, setTemplates] = useState<Template[]>([]);
  const [git, setGit] = useState<MirrorStatus | null>(null);
  const [history, setHistory] = useState<CommitEntry[]>([]);
  const [eveRunning, setEveRunning] = useState(false);
  const [patPresent, setPatPresent] = useState(false);
  const [portraits, setPortraits] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  const [info, setInfo] = useState<string | null>(null);
  const [update, setUpdate] = useState<UpdateCheck | null>(null);
  const accountsReq = useRef(0);

  // One-shot at startup, like nicotine's footer badge: green when
  // current, a red link when behind, nothing when the check fails.
  useEffect(() => {
    api.checkUpdate().then(setUpdate).catch(() => {});
  }, []);

  const [installingUpdate, setInstallingUpdate] = useState(false);

  // Clicking the red badge offers an in-place update. Anything that
  // prevents one (deb/rpm installs, a release without signed updater
  // artifacts, network trouble) falls back to the release page -
  // exactly what the badge did before the updater existed.
  const installUpdate = async () => {
    if (!update || installingUpdate) return;
    try {
      const yes = await ask(t("update_ask_body", { v: update.latest }), {
        title: t("update_ask_title"),
        kind: "info",
      });
      if (!yes) return;
      setInstallingUpdate(true);
      const u = await checkUpdaterFeed();
      if (!u) throw new Error("updater feed has no update");
      await u.downloadAndInstall();
      await relaunch();
    } catch {
      openUrl(update.url).catch(() => {});
    } finally {
      setInstallingUpdate(false);
    }
  };

  const refreshAll = useCallback(async () => {
    try {
      setError(null);
      const [inst, c, g, t, st, ep, pp] = await Promise.all([
        api.detectInstalls(),
        api.listCharacters(),
        api.listGroups(),
        api.listTemplates(),
        api.gitStatus(),
        api.isEveRunning(),
        // Belt to the backend's suspenders: a PAT probe must never take
        // the whole refresh down with it.
        api.patPresent().catch(() => false),
      ]);
      setInstalls(inst);
      setChars(c);
      setGroups(g);
      setTemplates(t);
      setGit(st);
      setEveRunning(ep);
      setPatPresent(pp);

      // The register parses launcher logs, which on a first run means
      // reading the whole active log; the roster must not wait on it.
      // Optional like the PAT probe, and a slow answer to an earlier
      // refresh must not overwrite a newer one.
      const req = ++accountsReq.current;
      api
        .listAccounts()
        .then((acc: AccountEntry[]) => {
          if (req === accountsReq.current) setAccounts(acc);
        })
        .catch(() => {});

      // All visible ids, not just unnamed ones: the backend diffs them
      // against its cache TTL, so renames re-resolve while a warm cache
      // costs one DB query and no network.
      const ids = c.characters.map((ch) => ch.id);
      if (ids.length) {
        // Fire-and-forget: portraits stream in whenever the cache or
        // the image server answers; the album shows monograms until
        // then and forever when offline.
        api.characterPortraits(ids)
          .then((p) => setPortraits((prev) => ({ ...prev, ...p })))
          .catch(() => {});
        try {
          const lookup = await api.resolveNames(ids);
          setChars((prev) =>
            prev
              ? {
                  ...prev,
                  characters: prev.characters.map((ch) => ({
                    ...ch,
                    name: lookup[String(ch.id)] ?? ch.name,
                  })),
                }
              : prev
          );
        } catch (e) {
          console.warn("ESI name resolution failed", e);
        }
      }

      try {
        setHistory(await api.gitHistory(50));
      } catch {
        setHistory([]);
      }
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refreshAll();
    const id = setInterval(() => {
      api.isEveRunning().then(setEveRunning).catch(() => {});
    }, 5000);
    return () => clearInterval(id);
  }, [refreshAll]);

  const charById = useMemo(() => {
    const m = new Map<number, CharacterEntry>();
    chars?.characters.forEach((c) => m.set(c.id, c));
    return m;
  }, [chars]);

  const counts: Partial<Record<Tab, number>> = {
    characters: chars?.characters.length ?? 0,
    accounts: accounts.length,
    groups: groups.length,
    templates: templates.length,
  };

  const installLine =
    installs && installs.installs.length > 0
      ? installs.installs.length === 1
        ? t("install_on_file")
        : t("installs_on_file", { n: installs.installs.length })
      : t("no_installs");

  return (
    <div className="app">
      <header className="masthead">
        <div className="masthead-side">
          <div
            className="cameo"
            onClick={() => api.playJazzChord().catch(() => {})}
            title="Take a chord"
          >
            <img src="/joecamel.png" alt="" />
          </div>
        </div>

        <div className="masthead-center">
          <div className="masthead-over">Turkish &amp; Domestic Blend</div>
          <h1 className="wordmark">Replicator</h1>
          <div className="masthead-under">{t("tagline")}</div>
        </div>

        <div className="masthead-side right">
          {eveRunning ? (
            <div className="sg-box">
              <strong>{t("sg_label")}</strong> {t("eve_running_blocked")}
            </div>
          ) : (
            <div className="masthead-note">{installLine}</div>
          )}
          <button className="ghost" onClick={refreshAll}>
            {t("refresh")}
          </button>
        </div>
      </header>

      <nav className="departments">
        {TABS.map((d) => (
          <button
            key={d.key}
            className={"dept" + (tab === d.key ? " active" : "")}
            onClick={() => switchTab(d.key)}
          >
            {t(d.label)}
            {counts[d.key] !== undefined && (
              <span className="dept-count">{counts[d.key]}</span>
            )}
          </button>
        ))}
        <div className="departments-fill" aria-hidden />
        <button
          className={"dept" + (tab === "about" ? " active" : "")}
          onClick={() => switchTab("about")}
        >
          {t("dept_about")}
        </button>
      </nav>

      <main className="page" ref={pageRef}>
        {error && (
          <Banner kind="bad" onDismiss={() => setError(null)}>
            {error}
          </Banner>
        )}
        {info && (
          <Banner kind="good" onDismiss={() => setInfo(null)}>
            {info}
          </Banner>
        )}

        {/* Every view stays mounted; inactive ones are display:none.
            Unmounting on tab switch threw away in-progress selections
            (source, targets, drawer state) the moment the user peeked
            at another tab - and warm layout caches make returns
            instant for free. */}
        <div hidden={tab !== "characters"}>
          <CharactersView
            chars={chars}
            groups={groups}
            templates={templates}
            portraits={portraits}
            eveRunning={eveRunning}
            onOpenSettings={() => switchTab("settings")}
            onAfterMutation={async (msg) => {
              setInfo(msg);
              await refreshAll();
            }}
            onError={setError}
          />
        </div>
        <div hidden={tab !== "accounts"}>
          <AccountsView
            accounts={accounts}
            characters={chars?.characters ?? []}
            portraits={portraits}
            onAfterMutation={async (msg) => {
              setInfo(msg);
              await refreshAll();
            }}
            onError={setError}
          />
        </div>
        <div hidden={tab !== "groups"}>
          <GroupsView
            groups={groups}
            charById={charById}
            characters={chars?.characters ?? []}
            onAfterMutation={async (msg) => {
              setInfo(msg);
              await refreshAll();
            }}
            onError={setError}
          />
        </div>
        <div hidden={tab !== "templates"}>
          <TemplatesView
            templates={templates}
            characters={chars?.characters ?? []}
            eveRunning={eveRunning}
            onAfterMutation={async (msg) => {
              setInfo(msg);
              await refreshAll();
            }}
            onError={setError}
          />
        </div>
        <div hidden={tab !== "git"}>
          <GitView
            status={git}
            history={history}
            patPresent={patPresent}
            eveRunning={eveRunning}
            onAfterMutation={async (msg) => {
              setInfo(msg);
              await refreshAll();
            }}
            onError={setError}
          />
        </div>
        <div hidden={tab !== "settings"}>
          <SettingsView
            installs={installs}
            patPresent={patPresent}
            onAfterMutation={async (msg) => {
              setInfo(msg);
              await refreshAll();
            }}
            onError={setError}
          />
        </div>
        <div hidden={tab !== "about"}>
          <AboutView />
        </div>
      </main>

      <footer className="footline">
        <div className="footline-links">
          <button
            className="footline-link"
            onClick={() =>
              openUrl("https://github.com/isomerc/replicator-redux").catch(
                () => {}
              )
            }
          >
            GitHub
          </button>
          <span className="footline-dot" aria-hidden>
            •
          </span>
          <button
            className="footline-link"
            onClick={() =>
              openUrl("https://www.illuminatedcorp.com").catch(() => {})
            }
          >
            {t("footer_recruiting")}
          </button>
        </div>
        {update &&
          (update.outdated ? (
            <button
              className="footline-badge outdated"
              disabled={installingUpdate}
              onClick={installUpdate}
            >
              {installingUpdate
                ? t("update_installing")
                : t("new_version_available", { v: update.latest })}
            </button>
          ) : (
            <span className="footline-badge current">
              {t("latest_version")}
            </span>
          ))}
      </footer>
    </div>
  );
}
