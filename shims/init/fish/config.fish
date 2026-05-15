# tabsh notification shim init
set -l user_config "$HOME/.config/fish/config.fish"
if test -f "$user_config"
  source "$user_config"
end
