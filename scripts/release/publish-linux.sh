#!/usr/bin/env bash
set -euo pipefail

: "${RELEASE_TAG:?}" "${RELEASE_VERSION:?}" "${RELEASE_SHA:?}" "${GH_REPO:?}"
assets=(
  "release-assets/OpsPier_${RELEASE_VERSION}_linux_x86_64.AppImage"
  "release-assets/OpsPier_${RELEASE_VERSION}_linux_x86_64.deb"
  "release-assets/SHA256SUMS-linux-x86_64.txt"
)
for asset in "${assets[@]}"; do test -s "$asset"; done
(cd release-assets && sha256sum -c SHA256SUMS-linux-x86_64.txt)

if ! gh release view "$RELEASE_TAG" --json tagName >/dev/null 2>&1; then
  notes="$RUNNER_TEMP/opspier-linux-release-notes.md"
  cat >"$notes" <<EOF
Linux x86_64 packages for OpsPier ${RELEASE_VERSION}.

Source tag: ${RELEASE_TAG}; commit: ${RELEASE_SHA}.
Built on Ubuntu 22.04. Use a compatible Linux desktop (glibc 2.35 or newer).

Download either the AppImage or Debian package and SHA256SUMS-linux-x86_64.txt.
Verify the downloaded package with its matching line from the checksum file.
For AppImage: chmod +x OpsPier_${RELEASE_VERSION}_linux_x86_64.AppImage, then run it.
For Debian/Ubuntu: sudo apt install ./OpsPier_${RELEASE_VERSION}_linux_x86_64.deb.

A desktop session with D-Bus and an unlocked Secret Service provider (for example GNOME Keyring) is required for saved credentials.
EOF
  latest=()
  if [[ "${IS_BACKFILL:-false}" == true ]]; then latest=(--latest=false); fi
  # Create as a draft so a new release is visible only after all assets exist.
  gh release create "$RELEASE_TAG" --verify-tag --draft --title "OpsPier ${RELEASE_TAG}" --notes-file "$notes" "${latest[@]}"
fi

gh release upload "$RELEASE_TAG" "${assets[@]}" --clobber

# Verify the uploaded set before publishing a newly created/existing draft.
remote_assets=$(gh release view "$RELEASE_TAG" --json assets --jq '.assets[].name')
for asset in "${assets[@]}"; do
  if ! grep -Fxq -- "${asset##*/}" <<<"$remote_assets"; then
    echo "Release is missing an uploaded Linux asset" >&2
    exit 1
  fi
done
if [[ "$(gh release view "$RELEASE_TAG" --json isDraft --jq '.isDraft')" == true ]]; then
  latest=()
  if [[ "${IS_BACKFILL:-false}" == true ]]; then latest=(--latest=false); fi
  gh release edit "$RELEASE_TAG" --draft=false "${latest[@]}"
fi
gh release view "$RELEASE_TAG" --json url --jq '.url'
