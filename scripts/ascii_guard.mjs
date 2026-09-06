#!/usr/bin/env node
// Character policy for the repository, enforced in CI.
//
//   src/i18n/           any typography - each locale keeps its own
//                       conventions (CJK punctuation, guillemets, ...)
//   src/ *.ts, *.tsx    ASCII plus the UI chrome glyphs the app
//                       renders: ellipsis, middle dot, bullet,
//                       multiplication sign
//   README.md           ASCII plus the middle-dot link separators
//   everything else     plain ASCII
//
// Ticket-tracker ids (the NIC- prefix) are internal and must not
// appear anywhere in the tree.
import { execSync } from "node:child_process";
import { readFileSync } from "node:fs";

const BINARY = /\.(png|woff2?|ttf|svg|ico|icns|dat)$/;
// Spelled as escapes so this file passes its own scan.
const UI_GLYPHS = new Set(["\u2026", "\u00b7", "\u2022", "\u00d7"]);
const TICKET = new RegExp("\\bNIC-\\d+\\b");

const files = execSync("git ls-files", { encoding: "utf8" })
  .trim()
  .split("\n")
  .filter((f) => !BINARY.test(f));

let bad = 0;
for (const f of files) {
  const i18n = f.startsWith("src/i18n/");
  const ui = !i18n && /^src\/.*\.(ts|tsx)$/.test(f);
  const readme = f === "README.md";
  readFileSync(f, "utf8")
    .split("\n")
    .forEach((line, i) => {
      if (TICKET.test(line)) {
        console.log(`${f}:${i + 1}: ticket reference: ${line.trim()}`);
        bad++;
      }
      if (i18n) return;
      for (const ch of line) {
        const cp = ch.codePointAt(0);
        if (cp <= 0x7f) continue;
        if (ui && UI_GLYPHS.has(ch)) continue;
        if (readme && ch === "\u00b7") continue;
        const hex = cp.toString(16).toUpperCase().padStart(4, "0");
        console.log(`${f}:${i + 1}: U+${hex} '${ch}': ${line.trim()}`);
        bad++;
        break; // one report per line keeps the output readable
      }
    });
}

if (bad) {
  console.error(`\n${bad} violation(s) of the character policy (see header).`);
  process.exit(1);
}
