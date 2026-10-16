#!/usr/bin/env bash
# Print the release notes for a tag, as Markdown, from the commits since the
# tag before it:
#   scripts/changelog.sh v0.7.0
# Subjects follow Conventional Commits (`feat(scope)!: text`): breaking
# changes, features and fixes get their own sections, everything else goes
# under "Other changes", and the `tm X.Y.Z` version bumps are left out.
set -euo pipefail
cd "$(dirname "$0")/.."

tag="$1"
previous="$(git describe --tags --abbrev=0 "$tag^" 2>/dev/null || true)"
range="${previous:+$previous..}$tag"
repo="${GITHUB_REPOSITORY:-$(git remote get-url origin | sed -E 's#^.*github\.com[:/]##; s#\.git$##')}"
url="https://github.com/$repo"

breaking=() features=() fixes=() other=()
while IFS=$'\t' read -r sha subject; do
  [[ $subject =~ ^tm\ [0-9] ]] && continue
  type="" scope="" bang="" text="$subject"
  if [[ $subject =~ ^([a-z]+)(\(([^\)]+)\))?(!)?:\ (.*)$ ]]; then
    type="${BASH_REMATCH[1]}" scope="${BASH_REMATCH[3]}"
    bang="${BASH_REMATCH[4]}" text="${BASH_REMATCH[5]}"
  fi
  entry="- ${scope:+**$scope:** }$text ([\`${sha:0:7}\`]($url/commit/$sha))"
  if [[ -n $bang ]] || git log -1 --format=%b "$sha" | grep -q '^BREAKING CHANGE'; then
    breaking+=("$entry")
  elif [[ $type == feat ]]; then
    features+=("$entry")
  elif [[ $type == fix ]]; then
    fixes+=("$entry")
  else
    other+=("$entry")
  fi
done < <(git log --reverse --format='%H%x09%s' "$range")

section() {
  local title="$1"
  shift
  (($#)) || return 0
  printf '## %s\n\n' "$title"
  printf '%s\n' "$@"
  printf '\n'
}
section "Breaking changes" ${breaking[@]+"${breaking[@]}"}
section "Features" ${features[@]+"${features[@]}"}
section "Fixes" ${fixes[@]+"${fixes[@]}"}
section "Other changes" ${other[@]+"${other[@]}"}
if [[ -n $previous ]]; then
  printf '**Full changelog**: %s/compare/%s...%s\n' "$url" "$previous" "$tag"
fi
