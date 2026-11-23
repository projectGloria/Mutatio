#!/usr/bin/env bash
set -euo pipefail
cd dist
tag="v$VERSION"
installer="Mutatio_${VERSION}_x64-setup.exe"
test -s "$installer"
sha256sum "$installer" > SHA256SUMS.txt

# Annotated tags have a peeled commit entry after the tag object entry.
refs=$(git ls-remote origin "refs/tags/$tag" "refs/tags/$tag^{}")
if [[ -n "$refs" && $(awk 'END {print $1}' <<< "$refs") != "$GITHUB_SHA" ]]; then
  echo "Tag $tag already points to another commit" >&2
  exit 1
fi

release=$(gh api --paginate "repos/$GITHUB_REPOSITORY/releases" | jq -sc --arg tag "$tag" 'add | map(select(.tag_name == $tag)) | first // empty')
if [[ -n "$release" ]] && jq -e '.draft == false' <<< "$release" >/dev/null; then
  verify_dir=$(mktemp -d)
  gh release download "$tag" --dir "$verify_dir" -p "$installer" -p SHA256SUMS.txt
  (cd "$verify_dir" && sha256sum "$installer" | diff - SHA256SUMS.txt)
  exit 0
fi
if [[ -z "$release" ]]; then
  options=()
  if [[ "$VERSION" == *-* ]]; then options+=(--prerelease); fi
  gh release create "$tag" --target "$GITHUB_SHA" --draft --title "Mutatio $VERSION" --generate-notes "${options[@]}"
elif [[ -z "$refs" && $(jq -r '.target_commitish' <<< "$release") != "$GITHUB_SHA" ]]; then
  echo "Draft targets a different commit" >&2
  exit 1
fi
gh release upload "$tag" "$installer" SHA256SUMS.txt --clobber
verify_dir=$(mktemp -d)
gh release download "$tag" --dir "$verify_dir" -p "$installer" -p SHA256SUMS.txt
cmp SHA256SUMS.txt "$verify_dir/SHA256SUMS.txt"
(cd "$verify_dir" && sha256sum --check SHA256SUMS.txt)
gh release edit "$tag" --draft=false
