#!/usr/bin/env node
// compute-version.mjs — decide the next version from the commit history.
//
// Reads the commit log since the previous `v*` tag and classifies it by
// Conventional Commit subject:
//   breaking (`!:`)            → major
//   `feat`                     → minor
//   anything else              → patch
// Bump commits written by this train (`[release-train]` in the subject) are
// excluded, so a re-run is idempotent: when nothing new has landed, the next
// version equals the current one and this script exits 0 printing "noop".
//
// Pure stdlib; no network; never writes. The workflow (release-train.yml) is
// the only intended caller, but the script is deliberately local-runnable:
//   PREV_TAG=v2.242.1 node scripts/release-train/compute-version.mjs
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..", "..");
const VERSION_FILE = path.join(ROOT, "panel", "agent", "Cargo.toml");

const BREAKING = /^(\w+)(\([\w.-]+\))?!:/;
const TYPE = /^(\w+)(\([\w.-]+\))?:(.*)$/;
const TRAIN = "[release-train]";

function currentVersion() {
  const line = readFileSync(VERSION_FILE, "utf8")
    .split("\n")
    .find((l) => l.startsWith("version = "));
  if (!line) throw new Error(`no 'version =' line in ${VERSION_FILE}`);
  return line.split('"')[1];
}

function previousTag() {
  if (process.env.PREV_TAG) return process.env.PREV_TAG;
  try {
    return execFileSync("git", [
      "describe", "--tags", "--abbrev=0", "--match", "v*", "HEAD",
    ], { cwd: ROOT, encoding: "utf8" }).trim();
  } catch {
    return null; // no tags yet: classify the whole history
  }
}

function subjectsSince(tag) {
  const range = tag ? `${tag}..HEAD` : "HEAD";
  let out;
  try {
    out = execFileSync("git", [
      "log", "--format=%s", range,
    ], { cwd: ROOT, encoding: "utf8" });
  } catch {
    // Tag exists but has no reachable ancestry diff (re-tag, shallow clone).
    return [];
  }
  return out.split("\n").filter(Boolean).filter((s) => !s.includes(TRAIN));
}

export function classify(subjects) {
  let breaking = 0, feat = 0, other = 0;
  for (const s of subjects) {
    if (BREAKING.test(s)) breaking++;
    else {
      const m = TYPE.exec(s);
      if (m && m[1] === "feat") feat++;
      else other++;
    }
  }
  return { breaking, feat, other };
}

export function nextVersion(current, { breaking, feat }) {
  const [maj, min, pat] = current.split(".").map(Number);
  if (breaking > 0) return `${maj + 1}.0.0`;
  if (feat > 0) return `${maj}.${min + 1}.0`;
  return `${maj}.${min}.${pat + 1}`;
}

const current = currentVersion();
const tag = previousTag();
const subjects = subjectsSince(tag);
const counts = classify(subjects);
const version = subjects.length === 0 ? current : nextVersion(current, counts);

// Machine-readable last line; the workflow greps it. Human summary above it.
console.log(`current: ${current}`);
console.log(`prev-tag: ${tag ?? "(none)"}`);
console.log(`commits: ${subjects.length} (breaking=${counts.breaking} feat=${counts.feat} other=${counts.other})`);
console.log(`next: ${version}`);
if (version === current) console.log("noop: no releasable commits since the previous tag");
