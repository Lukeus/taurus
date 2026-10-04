# Taurus shell integration, step 4 of 4: your own .zlogin, then ZDOTDIR goes
# back to what it was, so nothing started from this shell inherits Taurus's.
ZDOTDIR="$TAURUS_USER_ZDOTDIR"
[[ -f "$ZDOTDIR/.zlogin" ]] && source "$ZDOTDIR/.zlogin"
if [[ "$TAURUS_ZDOTDIR_WAS_SET" == 1 || "$TAURUS_USER_ZDOTDIR" != "$HOME" ]]; then
  export ZDOTDIR="$TAURUS_USER_ZDOTDIR"
else
  unset ZDOTDIR
fi
unset TAURUS_USER_ZDOTDIR TAURUS_ZDOTDIR TAURUS_ZDOTDIR_WAS_SET
