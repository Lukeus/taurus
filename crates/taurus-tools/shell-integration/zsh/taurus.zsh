# Taurus shell integration: the marks around each command.
#
#   ESC ] 133 ; A            a prompt starts
#   ESC ] 133 ; B            the prompt ends and typing starts
#   ESC ] 133 ; C ; cmdline_url=...   a command is about to run
#   ESC ] 133 ; D ; <status> it finished
#   ESC ] 7 ; file://host/dir         where the shell is
#
# The terminal draws none of them. Taurus reads them to split the scrollback
# into commands. This file only prints; it changes nothing about how your
# shell behaves.

[[ -o interactive ]] || return 0
(( ${+__taurus_hooked} )) && return 0
typeset -g __taurus_hooked=1

# Percent-encodes $1 into REPLY. Byte by byte, so any UTF-8 survives, and
# into a variable rather than printed, so no prompt costs a subshell.
__taurus_urlencode() {
  emulate -L zsh
  setopt localoptions noshwordsplit
  local LC_ALL=C s="$1" out="" c hex
  local -i i
  for (( i = 1; i <= ${#s}; i++ )); do
    c="${s[i]}"
    case "$c" in
      [a-zA-Z0-9._~/-]) out+="$c" ;;
      *) printf -v hex '%%%02X' "'$c"; out+="%${hex[-2,-1]}" ;;
    esac
  done
  REPLY="$out"
}

__taurus_precmd() {
  local code=$?
  # Sent every time, including after an empty Enter. Taurus ignores a status
  # when no command is running, so the shell doesn't need to know.
  builtin printf '\e]133;D;%s\a' "$code"
  __taurus_urlencode "$PWD"
  builtin printf '\e]7;file://%s%s\a' "${HOST}" "$REPLY"
  builtin printf '\e]133;A\a'
  return $code
}

# Last, so it sees the prompt every other hook has finished building. A
# prompt framework that rebuilds PS1 on each prompt gets the mark back here.
__taurus_prompt_end() {
  local code=$?
  if [[ "$PS1" != *$'\e]133;B\a'* ]]; then
    PS1="$PS1"$'%{\e]133;B\a%}'
  fi
  return $code
}

__taurus_preexec() {
  local cmd="$1"
  # A long paste is still one command. Past this it's labeled by its start.
  (( ${#cmd} > 2048 )) && cmd="${cmd[1,2048]}"
  __taurus_urlencode "$cmd"
  builtin printf '\e]133;C;cmdline_url=%s\a' "$REPLY"
}

# First in line, so its $? is the command's and not another hook's.
precmd_functions=(__taurus_precmd ${precmd_functions[@]} __taurus_prompt_end)
preexec_functions=(${preexec_functions[@]} __taurus_preexec)
