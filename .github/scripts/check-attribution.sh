#!/usr/bin/env bash
# Checks that every non-merge commit in <base>..<head> says which models helped write it.
#
# A commit passes when its trailers include, for each model used, either
#   Assisted-By: <model and version>
#     any model from any vendor, e.g. "Assisted-By: GPT-5.1" or "Assisted-By: Gemini 2.5 Pro", or
#   Co-Authored-By: <model and version> <noreply@vendor>
#     the form some tools write themselves, e.g.
#     "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>";
# or, for a commit written without any model,
#   Assisted-By: none
#
# A Co-Authored-By trailer counts as a model when its address is exactly noreply@<domain>;
# anything else is a person and is ignored. A model trailer without a version number, such
# as "Assisted-By: ChatGPT" or "Claude <noreply@anthropic.com>", fails: the point is to
# record which model it was.
#
# Usage: check-attribution.sh <base> <head>
#
# With ATTRIBUTION_REPORT set to a path, also appends one Markdown list item per failing
# commit to that file, for the PR comment. It holds only the commit hash and a fixed reason,
# never text from the commit, so a commit message can't put mentions or markup in the comment.
set -euo pipefail

if [ $# -ne 2 ]; then
  echo "usage: $0 <base> <head>" >&2
  exit 2
fi
base=$1
head=$2

example='"Assisted-By: <model and version>" (e.g. "Assisted-By: GPT-5.1") or "Co-Authored-By: <model and version> <noreply@vendor>"'

failed=0
checked=0

fail() {
  local commit=$1 subject=$2 reason=$3 summary=$4
  failed=$((failed + 1))
  echo "::error title=Missing model attribution::$(git rev-parse --short "$commit") \"$subject\": $reason"
  if [ -n "${ATTRIBUTION_REPORT:-}" ]; then
    echo "- $commit: $summary" >> "$ATTRIBUTION_REPORT"
  fi
}

trim() {
  printf '%s' "$1" | sed 's/^[[:space:]]*//; s/[[:space:]]*$//'
}

has_version() {
  printf '%s\n' "$1" | grep -q '[0-9]'
}

for commit in $(git rev-list --no-merges --reverse "$base..$head"); do
  checked=$((checked + 1))
  subject=$(git log -1 --format=%s "$commit")
  coauthors=$(git log -1 --format='%(trailers:key=Co-Authored-By,valueonly)' "$commit")
  assisted=$(git log -1 --format='%(trailers:key=Assisted-By,valueonly)' "$commit")

  models=0
  none=0
  bad=""

  while IFS= read -r value; do
    value=$(trim "$value")
    [ -z "$value" ] && continue
    # "Name <noreply@domain>" is a model or tool; any other address is a person.
    if printf '%s\n' "$value" | grep -Eiq '<noreply@[^<>@[:space:]]+>$'; then
      name=$(trim "${value%<*}")
      if has_version "$name"; then
        models=$((models + 1))
      else
        bad="Co-Authored-By: $value"
      fi
    fi
  done <<< "$coauthors"

  while IFS= read -r value; do
    value=$(trim "$value")
    [ -z "$value" ] && continue
    if printf '%s\n' "$value" | grep -Eiq '^none$'; then
      none=1
    elif has_version "$value"; then
      models=$((models + 1))
    else
      bad="Assisted-By: $value"
    fi
  done <<< "$assisted"

  if [ -n "$bad" ]; then
    fail "$commit" "$subject" "\"$bad\" doesn't say which version of the model; use $example" \
      "a model trailer doesn't say which version of the model"
  elif [ "$models" -gt 0 ] && [ "$none" -eq 1 ]; then
    fail "$commit" "$subject" "names a model and also says \"Assisted-By: none\"" \
      "names a model and also says \`Assisted-By: none\`"
  elif [ "$models" -eq 0 ] && [ "$none" -eq 0 ]; then
    fail "$commit" "$subject" "no model attribution; add $example for each model used, or \"Assisted-By: none\" if none was" \
      "no model attribution"
  fi
done

if [ "$failed" -gt 0 ]; then
  echo "$failed of $checked commit(s) don't attribute the models used."
  echo "Fix with an interactive rebase (reword), then force-push the branch."
  exit 1
fi
echo "All $checked commit(s) attribute the models used."
