# shellcheck shell=bash
# Shared by scripts/deploy.sh and the justfile: run a command on the server over ssh without depending on the
# remote login shell. Source it; it defines functions only.
#
# ssh joins the remote command words with spaces and the remote login shell re-splits the result, so each word
# is quoted locally with plain POSIX single quotes (valid in sh, dash and bash alike; no bash-only $'...').

# sq WORD: print WORD quoted for a POSIX shell (left bare when it is made only of safe characters).
sq() {
  if [[ "$1" =~ ^[A-Za-z0-9._/:=@%+,-]+$ ]]; then printf '%s' "$1"; else printf "'%s'" "${1//\'/\'\\\'\'}"; fi
}

# remote_ssh WORD...: run the words as one argv on $DEPLOY_SSH. Extra ssh options go in the ssh_opts array.
# stdin is passed through. Run `bash -c SCRIPT _ ARGS...` for anything that needs shell syntax.
remote_ssh() {
  local cmd="" w
  for w in "$@"; do cmd+="$(sq "$w") "; done
  # shellcheck disable=SC2029,SC2154  # ssh_opts is set by the caller;  # client-side expansion of $cmd is the point
  ssh "${ssh_opts[@]}" "$DEPLOY_SSH" "$cmd"
}

# deploy_dir_normalize DIR: print DIR relative to the login home (where ssh starts) or absolute. A leading "~/"
# is stripped and a bare "~" becomes "."; "~user/..." and anything outside [A-Za-z0-9._/-] is rejected.
deploy_dir_normalize() {
  local d="$1"
  # shellcheck disable=SC2088  # matching a literal "~/" prefix is the point
  case "$d" in
    "~") d="." ;;
    "~/"*) d="${d#\~/}"; d="${d:-.}" ;;
    "~"*) echo "bad DEPLOY_DIR '$1': ~user forms are not supported (use ~/dir, a relative path or an absolute path)" >&2; return 2 ;;
  esac
  if [[ ! "$d" =~ ^[A-Za-z0-9._/][A-Za-z0-9._/-]*$ ]]; then echo "bad DEPLOY_DIR '$1'" >&2; return 2; fi
  printf '%s\n' "$d"
}
