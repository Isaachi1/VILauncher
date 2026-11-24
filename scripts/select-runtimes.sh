#!/usr/bin/env bash
# Picks, from PortWatch VI's tracker, the most advanced effort with public code for each
# backend family VILauncher knows how to build:
#
#   victory: a VIctory tree (apps/victory/Cargo.toml), built with `cargo build -p victory`
#   xcl:     an XCL tree (crates/xcl/Cargo.toml), built with `cargo build -p xcl`
#
# The tracker only ranks; whether a repository is buildable is decided here, by looking for
# those files on its default branch. Efforts without public code, repositories that are gone
# (404 / 451 DMCA), archived, or of another family are skipped and listed with the reason.
# When no tracked effort of a family qualifies, the family falls back to the LPortVI fork.
#
# Usage: select-runtimes.sh TRACKER_JSON OUT_JSON
# Needs jq and an authenticated gh. Writes OUT_JSON and, under GitHub Actions, the job's
# outputs: victory_repo, victory_ref, victory_effort, xcl_repo, xcl_ref, xcl_effort, recommended.
set -euo pipefail

tracker=$1
out=$2
max=${SELECT_MAX_CANDIDATES:-20}

# Public efforts, most advanced first: a full milestone outweighs any render or fps
# (the same order as the launcher's Status tab).
candidates=$(jq -c --argjson max "$max" '
  [.efforts[]
   | select((.repo // "") != "")
   | .gta as $g
   | {repo, name: (.name // .id),
      rank: ((($g.loads // 0) + ($g.boots // 0) + ($g.menu // 0) + ($g.world // 0) + ($g.playable // 0) + ($g.story // 0)) * 1000
             + ($g.render // 0) * 100 + ([($g.fps // 0), 99] | min))}]
  | sort_by(-.rank, .name) | .[:$max] | .[]' "$tracker")

declare -A pick_repo pick_ref pick_effort pick_rank
log='[]'
note() { log=$(jq -c --arg repo "$1" --arg name "$2" --argjson rank "$3" --arg family "$4" --arg why "$5" \
  '. + [{repo: $repo, name: $name, rank: $rank, family: (if $family == "" then null else $family end), result: $why}]' <<<"$log"); }

while IFS= read -r c; do
  [ -n "$c" ] || continue
  repo=$(jq -r .repo <<<"$c"); name=$(jq -r .name <<<"$c"); rank=$(jq -r .rank <<<"$c")
  if [ -n "${pick_repo[victory]:-}" ] && [ -n "${pick_repo[xcl]:-}" ]; then break; fi
  if ! [[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]]; then note "$repo" "$name" "$rank" "" "not a GitHub owner/name"; continue; fi
  if ! info=$(gh api "repos/$repo" --jq '[.default_branch, .archived, .disabled] | @tsv' 2>/dev/null); then
    note "$repo" "$name" "$rank" "" "repository unavailable (gone or blocked)"; continue
  fi
  IFS=$'\t' read -r branch archived disabled <<<"$info"
  if [ "$archived" = true ] || [ "$disabled" = true ]; then note "$repo" "$name" "$rank" "" "archived or disabled"; continue; fi
  family=""
  if gh api "repos/$repo/contents/apps/victory/Cargo.toml?ref=$branch" --silent 2>/dev/null; then family=victory
  elif gh api "repos/$repo/contents/crates/xcl/Cargo.toml?ref=$branch" --silent 2>/dev/null; then family=xcl
  fi
  if [ -z "$family" ]; then note "$repo" "$name" "$rank" "" "no build recipe (not a VIctory or XCL tree)"; continue; fi
  if [ -n "${pick_repo[$family]:-}" ]; then note "$repo" "$name" "$rank" "$family" "behind ${pick_repo[$family]}"; continue; fi
  sha=$(gh api "repos/$repo/commits/$branch" --jq .sha)
  pick_repo[$family]=$repo; pick_ref[$family]=$sha; pick_effort[$family]=$name; pick_rank[$family]=$rank
  note "$repo" "$name" "$rank" "$family" "selected ($branch @ ${sha:0:12})"
done <<<"$candidates"

fallback() {  # family, repo
  [ -n "${pick_repo[$1]:-}" ] && return
  pick_repo[$1]=$2; pick_ref[$1]=$(gh api "repos/$2/commits/main" --jq .sha); pick_effort[$1]=""; pick_rank[$1]=-1
  note "$2" "$2" -1 "$1" "fallback: no tracked effort of this family qualified"
}
fallback victory LPortVI/VIctory
fallback xcl LPortVI/xcl

recommended=victory
if [ "$(jq -n "${pick_rank[xcl]} > ${pick_rank[victory]}")" = true ]; then recommended=xcl; fi

jq -n --arg at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --arg rec "$recommended" --argjson log "$log" \
  --arg vr "${pick_repo[victory]}" --arg vs "${pick_ref[victory]}" --arg ve "${pick_effort[victory]}" \
  --arg xr "${pick_repo[xcl]}" --arg xs "${pick_ref[xcl]}" --arg xe "${pick_effort[xcl]}" \
  '{selected_at: $at, recommended: $rec,
    victory: {repo: $vr, commit: $vs, effort: (if $ve == "" then null else $ve end)},
    xcl: {repo: $xr, commit: $xs, effort: (if $xe == "" then null else $xe end)},
    candidates: $log}' > "$out"

if [ -n "${GITHUB_OUTPUT:-}" ]; then
  {
    echo "victory_repo=${pick_repo[victory]}"; echo "victory_ref=${pick_ref[victory]}"; echo "victory_effort=${pick_effort[victory]}"
    echo "xcl_repo=${pick_repo[xcl]}"; echo "xcl_ref=${pick_ref[xcl]}"; echo "xcl_effort=${pick_effort[xcl]}"
    echo "recommended=$recommended"
  } >> "$GITHUB_OUTPUT"
fi
cat "$out"
