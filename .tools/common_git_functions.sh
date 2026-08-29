# Recursive git helpers for the repository group.
#
# Adapted from ihmcrobotics/repository-group (.tools/common_git_functions.sh).
# Changes: operate on the group root rather than $PWD, and prune build output so
# `find` does not walk target/ or node_modules/.

group_root() {
  cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd
}

# Function to check if a directory is a Git repository
is_git_repo() {
  git -C "$1" rev-parse --is-inside-work-tree &>/dev/null
}

# Returns the remote default branch name (e.g. develop, main).
get_default_branch() {
  local repo_dir="${1:?repo dir required}" remote="${2:-origin}"
  git -C "$repo_dir" symbolic-ref "refs/remotes/${remote}/HEAD" 2>/dev/null | sed 's|.*/||' \
    || git -C "$repo_dir" ls-remote --symref "$remote" HEAD 2>/dev/null \
    | awk '/^ref:/ { sub("refs/heads/", "", $2); print $2; exit }'
}

find_repo_dirs() {
  # 1. Recursively find .git folders, skipping build output.
  # 2. Remove the /.git from path.
  # 3. Make the path relative to the group root.
  # 4. Sort paths alphabetically.
  local root
  root="$(group_root)"
  find "$root" \( -name target -o -name node_modules \) -prune -o -type d -name ".git" -print0 \
  | xargs -0 -I {} dirname {} \
  | xargs -I {} realpath --relative-to="$root" {} \
  | sort
}

# Recursively runs git commands on all git repos in the group.
# Use _default_ in place of a branch name to use each repo's default branch.
git_recursive() {
  local root
  root="$(group_root)"
  mapfile -t repo_dirs < <(find_repo_dirs)

  for repo_dir in "${repo_dirs[@]}"; do
    if is_git_repo "$root/$repo_dir"; then
      local args=() arg
      for arg in "$@"; do
        if [ "$arg" = "_default_" ]; then
          arg=$(get_default_branch "$root/$repo_dir")
        fi
        args+=("$arg")
      done
      echo "git -C \"$repo_dir\" ${args[*]}"
      git -C "$root/$repo_dir" "${args[@]}"
    fi
  done
}

# Deletes branches that used to have a matching ref on the remote, but it has
# been deleted. This happens when PRs are merged.
git_recursive_delete_merged_branches() {
  local root
  root="$(group_root)"
  mapfile -t repo_dirs < <(find_repo_dirs)

  for repo_dir in "${repo_dirs[@]}"; do
    if is_git_repo "$root/$repo_dir"; then
      echo "git -C \"$repo_dir\" delete merged branches"
      git -C "$root/$repo_dir" for-each-ref --format '%(refname:short) %(upstream:track)' \
        | awk '$2 == "[gone]" {print $1}' \
        | xargs -r git -C "$root/$repo_dir" branch -D
    fi
  done
}
