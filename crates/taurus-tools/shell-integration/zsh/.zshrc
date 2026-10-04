# Taurus shell integration, step 3 of 4: your own .zshrc, then the hooks.
# The hooks go after it so they can sit outside whatever it installed.
ZDOTDIR="$TAURUS_USER_ZDOTDIR"
[[ -f "$ZDOTDIR/.zshrc" ]] && source "$ZDOTDIR/.zshrc"
ZDOTDIR="$TAURUS_ZDOTDIR"
[[ -f "$TAURUS_ZDOTDIR/taurus.zsh" ]] && source "$TAURUS_ZDOTDIR/taurus.zsh"
