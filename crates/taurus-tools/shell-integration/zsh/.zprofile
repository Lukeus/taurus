# Taurus shell integration, step 2 of 4: your own .zprofile.
ZDOTDIR="$TAURUS_USER_ZDOTDIR"
[[ -f "$ZDOTDIR/.zprofile" ]] && source "$ZDOTDIR/.zprofile"
ZDOTDIR="$TAURUS_ZDOTDIR"
