#!/usr/bin/env bash
set -euo pipefail

organization="https://dev.azure.com/JoiningClouds"
project="Tools"
repository="cli-gen"

source_branch=$(git symbolic-ref --quiet --short HEAD) || {
  echo "Check out the source branch before creating a PR." >&2
  exit 1
}
target_branch=$(git symbolic-ref --quiet --short refs/remotes/origin/HEAD) || {
  echo "Cannot determine origin's default branch. Run git remote set-head origin -a." >&2
  exit 1
}
target_branch=${target_branch#origin/}

if [[ "$source_branch" == "$target_branch" ]]; then
  echo "The source branch is already the target branch ($target_branch)." >&2
  exit 1
fi

remote_source_sha=$(git ls-remote --heads origin "refs/heads/$source_branch" | awk '{print $1}')
if [[ -z "$remote_source_sha" ]]; then
  echo "Branch $source_branch is not on origin. Push it before creating a PR." >&2
  exit 1
fi
if [[ "$remote_source_sha" != "$(git rev-parse HEAD)" ]]; then
  echo "Local $source_branch differs from origin/$source_branch. Sync it before creating a PR." >&2
  exit 1
fi

if ! git ls-remote --exit-code --heads origin "refs/heads/$target_branch" >/dev/null; then
  echo "Target branch $target_branch is not on origin." >&2
  exit 1
fi

pr_id=$(az repos pr list \
  --organization "$organization" \
  --project "$project" \
  --repository "$repository" \
  --source-branch "$source_branch" \
  --target-branch "$target_branch" \
  --status active \
  --query '[0].pullRequestId' \
  --output tsv)

if [[ -z "$pr_id" ]]; then
  pr_id=$(az repos pr create \
    --organization "$organization" \
    --project "$project" \
    --repository "$repository" \
    --source-branch "$source_branch" \
    --target-branch "$target_branch" \
    --title "Auto PR from $source_branch" \
    --description "Created via script" \
    --query pullRequestId \
    --output tsv)
  if [[ ! "$pr_id" =~ ^[0-9]+$ ]]; then
    echo "PR creation did not return a valid ID." >&2
    exit 1
  fi
  echo "Created PR #$pr_id from $source_branch to $target_branch."
else
  echo "Using existing PR #$pr_id from $source_branch to $target_branch."
fi

if az repos pr set-vote \
  --organization "$organization" \
  --id "$pr_id" \
  --vote approve \
  --output none; then
  echo "PR #$pr_id approved successfully."
else
  echo "Failed to approve PR #$pr_id." >&2
  exit 1
fi

if az repos pr update \
  --organization "$organization" \
  --id "$pr_id" \
  --auto-complete true \
  --output none; then
  echo "PR #$pr_id set to auto-complete successfully."
else
  echo "Failed to set auto-complete on PR #$pr_id." >&2
  exit 1
fi
