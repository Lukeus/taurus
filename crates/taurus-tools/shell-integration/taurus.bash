# Taurus shell integration for bash: your own startup files, then the marks
# around each command. See zsh/taurus.zsh for what the marks are.
#
# Started with --init-file, which bash reads in place of ~/.bashrc. So this
# reads what bash itself would have: the profile files for a login shell
# (which is what Taurus starts, as Terminal.app does), or ~/.bashrc.

if [[ "$TAURUS_SHELL_LOGIN" == 1 ]]; then
  [[ -r /etc/profile ]] && . /etc/profile
  for __taurus_f in ~/.bash_profile ~/.bash_login ~/.profile; do
    if [[ -r "$__taurus_f" ]]; then . "$__taurus_f"; break; fi
  done
  unset __taurus_f
else
  [[ -r ~/.bashrc ]] && . ~/.bashrc
fi
unset TAURUS_SHELL_LOGIN

if [[ $- == *i* && -z "$__taurus_hooked" ]]; then
__taurus_hooked=1

# Percent-encodes $1 into REPLY, byte by byte.
__taurus_urlencode() {
  local LC_ALL=C s="$1" out="" c hex i
  for (( i = 0; i < ${#s}; i++ )); do
    c="${s:i:1}"
    case "$c" in
      [a-zA-Z0-9._~/-]) out+="$c" ;;
      *) printf -v hex '%%%02X' "'$c"; out+="%${hex: -2}" ;;
    esac
  done
  REPLY="$out"
}

# The history number at the last prompt. A command that didn't advance it
# wasn't recorded — HISTCONTROL=ignorespace, a HISTIGNORE pattern — and its
# line isn't reported either: `history 1` would name the one before it, and a
# leading space is how people say "don't keep this".
__taurus_histnum() {
  local line
  line=$(HISTTIMEFORMAT= builtin history 1)
  line="${line#"${line%%[![:space:]]*}"}"
  REPLY="${line%%[!0-9]*}"
}

__taurus_command_start() {
  local num cmd
  __taurus_histnum; num="$REPLY"
  if [[ -n "$num" && "$num" != "$__taurus_last_hist" ]]; then
    cmd=$(HISTTIMEFORMAT= builtin history 1)
    cmd="${cmd#"${cmd%%[![:space:]]*}"}"
    cmd="${cmd#"$num"}"
    cmd="${cmd#"${cmd%%[![:space:]]*}"}"
    cmd="${cmd:0:2048}"
    __taurus_urlencode "$cmd"
    builtin printf '\e]133;C;cmdline_url=%s\a' "$REPLY"
  else
    builtin printf '\e]133;C\a'
  fi
}

__taurus_precmd() {
  local code=$?
  __taurus_at_prompt=
  builtin printf '\e]133;D;%s\a' "$code"
  __taurus_urlencode "$PWD"
  builtin printf '\e]7;file://%s%s\a' "$HOSTNAME" "$REPLY"
  builtin printf '\e]133;A\a'
  return $code
}

__taurus_prompt_end() {
  local code=$?
  case "$PS1" in
    *'\[\e]133;B\a\]'*) ;;
    *) PS1="$PS1"'\[\e]133;B\a\]' ;;
  esac
  __taurus_histnum; __taurus_last_hist="$REPLY"
  __taurus_at_prompt=1
  return $code
}

if (( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 4) )); then
  # PS0 is printed after a line is read and before it runs: exactly where C
  # goes. It runs in a subshell, which is why nothing here sets state.
  PS0='$(__taurus_command_start)'"$PS0"
elif [[ -z "$(trap -p DEBUG)" ]]; then
  # Older bash (macOS ships 3.2) has no PS0. The DEBUG trap runs before every
  # simple command; the flag makes only the first one after a prompt count.
  __taurus_debug() {
    if [[ -n "$__taurus_at_prompt" ]]; then
      __taurus_at_prompt=
      __taurus_command_start
    fi
  }
  trap '__taurus_debug' DEBUG
fi
# A DEBUG trap of your own is left alone, and commands go unmarked instead.

# Ours first, so its $? is the command's; then yours; then the prompt end.
if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null)" == "declare -a"* ]]; then
  PROMPT_COMMAND=(__taurus_precmd "${PROMPT_COMMAND[@]}" __taurus_prompt_end)
else
  PROMPT_COMMAND="__taurus_precmd${PROMPT_COMMAND:+; $PROMPT_COMMAND}; __taurus_prompt_end"
fi

fi
