if set -q TABSH_SHIM_DIR
  set -gx PATH $TABSH_SHIM_DIR $PATH
end

function __tabsh_osc7 --on-event fish_prompt
  printf '\e]7;file://%s%s\a' (hostname) (string replace -a '%2F' '/' (string escape --style=url -- $PWD))
  printf '\e]777;cmd;\a'
end

function __tabsh_preexec --on-event fish_preexec
  printf '\e]777;cmd;%s\a' (string escape --style=url -- "$argv")
end
