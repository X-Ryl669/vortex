#!/usr/bin/env bash
# Publish a SNAPSHOT of this private dev repo (vortex-lab) to the public
# mirror repo — the Linux-kernel model: development history stays private,
# the public repo receives curated release snapshots as single commits.
#
#   scripts/publish.sh                 # push a snapshot to public main
#   scripts/publish.sh v1.0.0-beta.2   # snapshot + tag → triggers the
#                                      # public repo's release workflow
#                                      # (signed APK on GitHub Releases)
#
# What it does:
#   1. clones the public repo shallowly into a temp dir (or inits it on
#      the very first publish),
#   2. replaces its tree with the current HEAD tree of this repo, minus
#      scripts/publish-exclude.txt (internal docs, secrets, artifacts),
#   3. README.en.md → README.md (public face is English; the uz README and
#      all project docs stay private — public = code only),
#   4. commits as ONE snapshot commit + pushes (and tags, if asked).
#
# Requirements: git push access to $PUBLIC_REMOTE (SSH key). The public
# repo must already exist on GitHub (create once: private→public there).
set -euo pipefail

PUBLIC_REMOTE=${PUBLIC_REMOTE:-git@github.com:zoir-dev/vortex.git}
TAG=${1:-}

# Resolve the dev repo from the script's own location (not the caller's
# cwd — running this from some other directory must not snapshot THAT).
SRC=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && git rev-parse --show-toplevel)
SRC_HEAD=$(git -C "$SRC" rev-parse --short HEAD)
if [[ -n $(git -C "$SRC" status --porcelain) ]]; then
  echo "✗ working tree is dirty — commit first (the snapshot mirrors HEAD)" >&2
  exit 1
fi
[[ -f "$SRC/README.en.md" ]] || { echo "✗ README.en.md missing" >&2; exit 1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

echo "→ fetching public repo…"
if git clone --depth 1 "$PUBLIC_REMOTE" "$TMP/pub" 2>/dev/null; then
  :
else
  echo "  (empty/new public repo — first publish)"
  git -C "$TMP" init -q -b main pub
  git -C "$TMP/pub" remote add origin "$PUBLIC_REMOTE"
fi

# Replace the tree wholesale (deletions in the dev repo propagate too).
find "$TMP/pub" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
# Export HEAD (not the working dir) so only committed content ships.
git -C "$SRC" archive HEAD | tar -x -C "$TMP/pub"
# Then drop the excluded paths. (git-archive already omits everything
# gitignored — the artifact patterns here are belt & braces.)
while IFS= read -r pat; do
  [[ -z "$pat" || "$pat" == \#* ]] && continue
  if [[ "$pat" == *"*"* ]]; then
    find "$TMP/pub" -name "$pat" -prune -exec rm -rf {} + 2>/dev/null || true
  else
    rm -rf "${TMP:?}/pub/${pat}" 2>/dev/null || true
    find "$TMP/pub" -mindepth 2 -type d -name "$pat" -prune -exec rm -rf {} + 2>/dev/null || true
  fi
done < "$SRC/scripts/publish-exclude.txt"

# Public face is English-only (the uz README is excluded with the docs —
# the public repo ships code + README/LICENSE/CONTRIBUTING, nothing else).
mv "$TMP/pub/README.en.md" "$TMP/pub/README.md"

cd "$TMP/pub"
git add -A
if git diff --cached --quiet; then
  echo "✓ public repo already up to date — nothing to publish"
else
  MSG="release: snapshot ${TAG:-$(date +%Y-%m-%d)} (dev @${SRC_HEAD})"
  git commit -q -m "$MSG"
  git push origin main
  echo "✓ pushed: $MSG"
fi

if [[ -n "$TAG" ]]; then
  git tag -f "$TAG"
  git push -f origin "$TAG"
  echo "✓ tagged $TAG → the public repo's release workflow builds the signed APK"
fi
