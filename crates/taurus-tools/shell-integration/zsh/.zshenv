# Taurus shell integration, step 1 of 4. Your own .zshenv runs here, then
# zsh is pointed back at Taurus's directory so the next file it reads is the
# next step. See crates/taurus-tools/src/shell_integration.rs for why it is done this way.
ZDOTDIR="${TAURUS_USER_ZDOTDIR:-$HOME}"
[[ -f "$ZDOTDIR/.zshenv" ]] && source "$ZDOTDIR/.zshenv"
# A .zshenv that sets ZDOTDIR is saying where the rest of its files are.
TAURUS_USER_ZDOTDIR="$ZDOTDIR"
ZDOTDIR="$TAURUS_ZDOTDIR"
