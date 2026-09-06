import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useI18n } from "../i18n";

const DOCS = "https://replicator.rip/docs";
const SITE = "https://replicator.rip";
const REPO = "https://github.com/isomerc/replicator";
const DISCORD = "https://discord.gg/N82KJcS47f";

/** The back of the pack: links, credits, small print. */
export function AboutView() {
  const { t } = useI18n();
  const [version, setVersion] = useState("");

  useEffect(() => {
    getVersion().then(setVersion).catch(() => {});
  }, []);

  const open = (url: string) => () => openUrl(url).catch(() => {});

  return (
    <div className="colophon">
      <img className="colophon-ornament" src="/ornament.svg" alt="" />

      <div className="colophon-kicker">Turkish &amp; Domestic Blend</div>
      <div className="wordmark">Replicator</div>
      <div className="colophon-version">
        {version ? t("version_line", { v: version }) : " "} ·{" "}
        {t("mit_licensed")}
      </div>

      <div className="colophon-links">
        <button className="primary" onClick={open(DOCS)}>
          {t("about_docs")}
        </button>
        <button onClick={open(REPO)}>GitHub</button>
        <button onClick={open(DISCORD)}>Discord</button>
      </div>
      <button className="ghost colophon-site" onClick={open(SITE)}>
        replicator.rip
      </button>

      <div className="colophon-rule" aria-hidden />

      <div className="colophon-credits">
        <p>{t("about_credit_author")}</p>
        <p>{t("about_credit_fonts")}</p>
        <p>{t("about_credit_network")}</p>
        <p>{t("about_credit_trademark")}</p>
      </div>

      {/* Part of the pack art, not the translatable chrome. */}
      <div className="colophon-warning">
        <strong>SURGEON GENERAL'S WARNING:</strong> This software has been
        determined by the New Eden Surgeon General to cause addiction to a
        properly arranged UI. Use of Replicator may result in permanent
        inability to blame the client for your losses.
      </div>
    </div>
  );
}
