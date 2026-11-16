#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Brings this repository (fjenn/wispr-bro-publik) up to date with
# fjenn/wispr-bro: merges its main into ours and pushes. fjenn/wispr-bro is
# only read, never written. Afterwards the two differ only by the publik
# layer, the files in publik/layer-files.txt; anything else stops the sync
# before the push. A merge conflict also stops it (nothing is pushed).
#
# Runs hourly in .github/workflows/wb-publik-sync.yml and by hand:
#   publik/sync.sh
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
MAIN_URL="${WB_SYNC_MAIN_URL:-https://github.com/fjenn/wispr-bro.git}"
die() { echo "sync: $*" >&2; exit 1; }

[[ -z "$(git status --porcelain)" ]] || die "uncommitted changes"
git fetch -q origin main
git checkout -q main
git merge -q --ff-only origin/main
git fetch -q "$MAIN_URL" main
UPSTREAM="$(git rev-parse FETCH_HEAD)"

# A publik-only version (add-publik.sh: 1.0.0-alpha.7.2 on main while
# fjenn/wispr-bro is at 1.0.0-alpha.7) gives way when fjenn/wispr-bro releases
# a new version: main goes back to the version both had, then the merge brings
# the new one in without a conflict.
version_at() { git show "$1:src-tauri/tauri.fork.conf.json" | python3 -Ic 'import json,sys; print(json.load(sys.stdin)["version"])'; }
BASE="$(git merge-base HEAD "$UPSTREAM")"
V_OURS="$(version_at HEAD)"; V_BASE="$(version_at "$BASE")"; V_UP="$(version_at "$UPSTREAM")"
if [[ "$V_OURS" != "$V_BASE" && "$V_UP" != "$V_BASE" ]]; then
  python3 -I publik/set-version.py "$V_OURS" "$V_BASE"
  git commit -q -am "sync: back to ${V_BASE} before fjenn/wispr-bro ${V_UP}"
  echo "sync: publik version ${V_OURS} gives way to fjenn/wispr-bro ${V_UP}"
fi

if git merge-base --is-ancestor "$UPSTREAM" HEAD; then
  echo "sync: already has fjenn/wispr-bro ${UPSTREAM:0:8}"
else
  git merge -q --no-edit -m "sync: fjenn/wispr-bro ${UPSTREAM:0:8}" "$UPSTREAM" \
    || { git merge --abort; die "fjenn/wispr-bro ${UPSTREAM:0:8} conflicts with the publik layer; merge it by hand"; }
fi

# The only differences from fjenn/wispr-bro are the publik layer.
# A publik-only version may differ in the version lines of the four version
# files; anything else in them counts.
VERSION_FILES="package.json src-tauri/tauri.fork.conf.json src-tauri/Cargo.toml src-tauri/Cargo.lock"
version_only() {
  git diff -U0 "$UPSTREAM" HEAD -- "$1" | grep -E '^[+-]' | grep -vE '^(\+\+\+|---) ' \
    | grep -vE '^[+-][[:space:]]*("version": "[^"]*",?|version = "[^"]*")$' | grep -q . && return 1
  return 0
}
extra=""
for f in $(git diff --name-only "$UPSTREAM" HEAD | grep -vxF -f publik/layer-files.txt || true); do
  if [[ " $VERSION_FILES " == *" $f "* ]] && version_only "$f"; then continue; fi
  extra+="${f}"$'\n'
done
[[ -z "$extra" ]] || die "differs from fjenn/wispr-bro outside the publik layer:
${extra}"

git push -q origin HEAD:main
echo "sync: main has fjenn/wispr-bro ${UPSTREAM:0:8} plus the publik layer"
