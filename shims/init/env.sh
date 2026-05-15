# tabsh notification shim init
[ -n "$TABSH_ORIG_ENV" ] && . "$TABSH_ORIG_ENV"
if [ -n "$TABSH_SHIM_DIR" ]; then
  export PATH="$TABSH_SHIM_DIR:$PATH"
fi
