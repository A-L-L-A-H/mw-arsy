#!/usr/bin/env node
// apply-release.mjs — write the release: version files, doc stamps, changelog.
//
// The 8-file write surface is pinned by tests/docs-claims-pin-e2e.sh and
// scripts/release.sh's consistency check:
//   panel/agent/Cargo.toml      ^version = "X"
//   panel/backend/Cargo.toml    ^version = "X"
//   panel/cli/Cargo.toml        ^version = "X"
//   panel/mcp/Cargo.toml        ^version = "X"
//   panel/frontend/package.json "version": "X"
//   FEATURES.md                 > **Version**: vX.Y.Z | ...
//   docs/testing.md             > **Reflects vX.Y.Z.** ...
//   CHANGELOG.md                new `## [X.Y.Z]` section at the top
//
// Each replacement touches ONE line (regex anchored to the exact stamp shape),
// so nothing else in those files can drift. Idempotent: when the version files
// already hold TARGET, nothing is written and the script exits 0 — the
// workflow treats that as "no release to cut".
//
// Usage: TARGET=2.243.0 PREV_TAG=v2.242.1 node scripts/release-train/apply-release.mjs
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..", "..");
const TARGET = process.env.TARGET;
if (!TARGET || !/^\d+\.\d+\.\d+$/.test(TARGET)) {
  console.error("apply-release: set TARGET=X.Y.Z (no v prefix)");
  process.exit(1);
}

const files = {
  agent: path.join(ROOT, "panel", "agent", "Cargo.toml"),
  backend: path.join(ROOT, "panel", "backend", "Cargo.toml"),
  cli: path.join(ROOT, "panel", "cli", "Cargo.toml"),
  mcp: path.join(ROOT, "panel", "mcp", "Cargo.toml"),
  frontend: path.join(ROOT, "panel", "frontend", "package.json"),
  features: path.join(ROOT, "FEATURES.md"),
  testing: path.join(ROOT, "docs", "testing.md"),
  changelog: path.join(ROOT, "CHANGELOG.md"),
};

const read = (p) => readFileSync(p, "utf8");

// Replace ONLY the first `^version = "..."` line; a crate can legitimately
// quote other version strings, and the dependency tables live below it.
function setTomlVersion(p, v) {
  const text = read(p);
  const next = text.replace(/^version = "[^"]+"/m, `version = "${v}"`);
  if (next === text) throw new Error(`no version line replaced in ${p}`);
  return next;
}

function setPackageVersion(p, v) {
  const text = read(p);
  const next = text.replace(/^(\s*)"version": "[^"]+"/m, `$1"version": "${v}"`);
  if (next === text) throw new Error(`no version line replaced in ${p}`);
  return next;
}

// The docs-claims pin greps the FIRST `**Version**: vX.Y.Z` occurrence and the
// `> **Reflects vX.Y.Z.**` stamp; keep those shapes byte-exact.
function setFeaturesStamp(p, v) {
  const text = read(p);
  const next = text.replace(/\*\*Version\*\*: v\d+\.\d+\.\d+/, `**Version**: v${v}`);
  if (next === text) throw new Error(`no **Version** stamp found in ${p}`);
  return next;
}

function setTestingStamp(p, v) {
  const text = read(p);
  const next = text.replace(/> \*\*Reflects v\d+\.\d+\.\d+\.\*\*/, `> **Reflects v${v}.**`);
  if (next === text) throw new Error(`no **Reflects** stamp found in ${p}`);
  return next;
}

function commitSubjects() {
  let tag = process.env.PREV_TAG;
  if (!tag) {
    try {
      tag = execFileSync("git", ["describe", "--tags", "--abbrev=0", "--match", "v*", "HEAD"],
        { cwd: ROOT, encoding: "utf8" }).trim();
    } catch { tag = null; }
  }
  try {
    const out = execFileSync("git", ["log", "--format=%s", tag ? `${tag}..HEAD` : "HEAD"],
      { cwd: ROOT, encoding: "utf8" });
    return out.split("\n").filter(Boolean).filter((s) => !s.includes("[release-train]"));
  } catch {
    return [];
  }
}

function changelogSection(v, subjects) {
  const today = new Date().toISOString().slice(0, 10);
  const lines = [];
  if (subjects.length === 0) {
    lines.push("Maintenance release: dependency and infrastructure updates only.");
  } else {
    for (const s of subjects.slice(0, 40)) lines.push(`- ${s}`);
    if (subjects.length > 40) lines.push(`- …and ${subjects.length - 40} more`);
  }
  return [`## [${v}]`, "", `### Changes (${today})`, "", ...lines, ""].join("\n");
}

function setChangelog(p, v, subjects) {
  const text = read(p);
  const header = text.startsWith("# Changelog") ? text.split("\n").slice(0, 4).join("\n") : "";
  const rest = header ? text.slice(header.length) : text;
  const section = changelogSection(v, subjects);
  return header ? header + "\n" + section + rest.replace(/^\n+/, "\n") : section + "\n" + rest;
}

// ---- Idempotence guard ----------------------------------------------------
const agentVersion = read(files.agent).split("\n").find((l) => l.startsWith("version = ")).split('"')[1];
if (agentVersion === TARGET) {
  console.log(`noop: version is already ${TARGET}; nothing written`);
  process.exit(0);
}

// ---- Write ----------------------------------------------------------------
const subjects = commitSubjects();
let writes = 0;
const done = [];
const write = (p, next, label) => {
  const before = read(p);
  if (before !== next) { writeFileSync(p, next); writes++; done.push(label); }
};

write(files.agent, setTomlVersion(files.agent, TARGET), "panel/agent/Cargo.toml");
write(files.backend, setTomlVersion(files.backend, TARGET), "panel/backend/Cargo.toml");
write(files.cli, setTomlVersion(files.cli, TARGET), "panel/cli/Cargo.toml");
write(files.mcp, setTomlVersion(files.mcp, TARGET), "panel/mcp/Cargo.toml");
write(files.frontend, setPackageVersion(files.frontend, TARGET), "panel/frontend/package.json");
write(files.features, setFeaturesStamp(files.features, TARGET), "FEATURES.md stamp");
write(files.testing, setTestingStamp(files.testing, TARGET), "docs/testing.md stamp");
write(files.changelog, setChangelog(files.changelog, TARGET, subjects), "CHANGELOG.md");

console.log(`applied v${TARGET}: ${writes} file(s) written`);
for (const d of done) console.log(`  - ${d}`);
