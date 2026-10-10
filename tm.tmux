#!/usr/bin/env bash
# tm - tmux session and window switcher on fzf.
# Load it with TPM:
#   set -g @plugin 'zhu-jiyuan/tmux-manager'
# or from ~/.tmux.conf directly:
#   run-shell ~/path/to/tmux-manager/tm.tmux
# Options go before that line:
#   set -g @tm-bin           "/path/to/tm"    # default: tm next to this script or target/release/tm, else a fetched release (checkouts), else ~/.cargo/bin/tm or PATH
#   set -g @tm-session-key   "s"              # prefix + key: session list (replaces choose-tree -s)
#   set -g @tm-window-key    "w"              # prefix + key: window list  (replaces choose-tree -w)
#   set -g @tm-switch-width  "75%"            # popup size, columns or percent
#   set -g @tm-switch-height "65%"
#   set -g @tm-projects      "~/work ~/oss:2"  # project roots, scanned 1 level deep (or :N); enables the project list
#   set -g @tm-project-key   "f"              # prefix + key: project list (replaces find-window), only with @tm-projects

CURRENT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

get_opt() {
  local value
  value="$(tmux show-option -gqv "$1")"
  printf '%s' "${value:-$2}"
}

find_bin() {
  for candidate in "$@"; do
    if [ -n "$candidate" ] && [ -x "$candidate" ]; then
      bin="$candidate"
      return
    fi
  done
}

bin="$(get_opt "@tm-bin" "")"
# A release tarball puts the binary next to this script; a developer's checkout has a build.
[ -n "$bin" ] || find_bin "$CURRENT_DIR/tm" "$CURRENT_DIR/target/release/tm"

# A checkout (TPM clones one) has neither: fetch the release for this platform
# next to this script, and fetch again when the checkout moves to another
# version. A symlinked tm is a developer's own and is left alone.
fetched="$CURRENT_DIR/tm"
version="$(grep -m1 '^version' "$CURRENT_DIR/Cargo.toml" 2>/dev/null | cut -d'"' -f2)"
if [ -n "$version" ] && [ ! -L "$fetched" ]; then
  if [ -z "$bin" ] || { [ "$bin" = "$fetched" ] && [ "$("$bin" --version 2>/dev/null)" != "tm $version" ]; }; then
    if "$CURRENT_DIR/scripts/install.sh"; then
      bin="$fetched"
      tmux display-message "tm $version installed"
    elif [ -z "$bin" ]; then
      tmux display-message "tm: could not install the binary; see scripts/install.sh or set @tm-bin"
    fi
  fi
fi

# Last resorts: a binary installed some other way.
[ -n "$bin" ] || find_bin "$HOME/.cargo/bin/tm" "$(command -v tm 2>/dev/null)"
if [ -z "$bin" ]; then
  tmux display-message "tm: binary not found; run cargo build --release or set @tm-bin"
  exit 0
fi

# display-popup does not expand formats in its command (tmux 3.4), so run-shell
# fills in the client name and opens the popup on that client. Its look is the
# user's: popup-style, popup-border-style, popup-border-lines.
size="-w '$(get_opt "@tm-switch-width" "75%")' -h '$(get_opt "@tm-switch-height" "65%")'"

bind_popup() {
  # $1 key, $2 extra tm arguments
  tmux bind-key "$1" run-shell -b "tmux display-popup -c '#{q:client_name}' -E -T ' tm ' $size \"'$bin' switch '#{q:client_name}' $2\""
}

bind_popup "$(get_opt "@tm-session-key" "s")" ""
bind_popup "$(get_opt "@tm-window-key" "w")" "--windows"
if [ -n "$(get_opt "@tm-projects" "")" ]; then
  bind_popup "$(get_opt "@tm-project-key" "f")" "--projects"
fi
