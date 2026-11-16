#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
#
# Releases the publik edition on GitHub's Macs (.github/workflows/
# wb-publik-release.yml): the tag goes on main, GitHub builds, signs,
# notarizes, self-tests, attests and publishes both chips.
#
#   publik/release.sh v1.0.0-alpha.8      after fjenn/wispr-bro released it
#                                         (and the sync brought it in)
#   publik/release.sh v1.0.0-alpha.7.3    a publik-only version of
#                                         fjenn/wispr-bro's v1.0.0-alpha.7
#
# The first listing, the app token and local builds stay with add-publik.sh.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
TAG="${1:-}"
die() { echo "release.sh: $*" >&2; exit 1; }
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)\.[0-9]+(\.[0-9]+)?)?$ ]] || die "usage: publik/release.sh v<version>"
VERSION="${TAG#v}"
[[ "$(git rev-parse --abbrev-ref HEAD)" == main ]] || die "check out main first"
[[ -z "$(git status --porcelain)" ]] || die "uncommitted changes"
git fetch -q origin main --tags
[[ "$(git rev-parse HEAD)" == "$(git rev-parse origin/main)" ]] || die "main is not origin/main; pull or push first"
git rev-parse -q --verify "refs/tags/${TAG}" >/dev/null && die "${TAG} exists already"
gh secret list -R fjenn/wispr-bro-publik | grep -q '^APPLE_CERTIFICATE' || die "the signing secrets are not set (publik/README.md)"

HERE="$(python3 -Ic 'import json; print(json.load(open("src-tauri/tauri.fork.conf.json"))["version"])')"
if [[ "$HERE" != "$VERSION" ]]; then
  # Only a publik-only version (.N on the release main is on) is set here.
  [[ "$VERSION" =~ ^(.+-(alpha|beta|rc)\.[0-9]+)\.[0-9]+$ ]] || die "main is at ${HERE}, not ${VERSION}"
  BASE="${BASH_REMATCH[1]}"
  [[ "$HERE" == "$BASE" || "$HERE" == "${BASE}."* ]] || die "main is at ${HERE}; ${TAG} is not a publik-only version of it"
  python3 -I publik/set-version.py "$HERE" "$VERSION"
  git commit -q -am "release: publik edition ${VERSION}"
  git push -q origin HEAD:main
fi
git tag "$TAG"
git push -q origin "refs/tags/${TAG}"
echo "release.sh: ${TAG} pushed; GitHub builds it: https://github.com/fjenn/wispr-bro-publik/actions/workflows/wb-publik-release.yml"
