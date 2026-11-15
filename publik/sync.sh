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
MAIN_URL="https://github.com/fjenn/wispr-bro.git"
die() { echo "sync: $*" >&2; exit 1; }

[[ -z "$(git status --porcelain)" ]] || die "uncommitted changes"
git fetch -q origin main
git checkout -q main
git merge -q --ff-only origin/main
git fetch -q "$MAIN_URL" main
UPSTREAM="$(git rev-parse FETCH_HEAD)"

if git merge-base --is-ancestor "$UPSTREAM" HEAD; then
  echo "sync: already has fjenn/wispr-bro ${UPSTREAM:0:8}"
else
  git merge -q --no-edit -m "sync: fjenn/wispr-bro ${UPSTREAM:0:8}" "$UPSTREAM" \
    || { git merge --abort; die "fjenn/wispr-bro ${UPSTREAM:0:8} conflicts with the publik layer; merge it by hand"; }
fi

# The only differences from fjenn/wispr-bro are the publik layer.
extra="$(git diff --name-only "$UPSTREAM" HEAD | grep -vxF -f publik/layer-files.txt || true)"
[[ -z "$extra" ]] || die "differs from fjenn/wispr-bro outside the publik layer:
${extra}"

git push -q origin HEAD:main
echo "sync: main has fjenn/wispr-bro ${UPSTREAM:0:8} plus the publik layer"
