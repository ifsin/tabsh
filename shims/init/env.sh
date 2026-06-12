if [ -n "$TABSH_ORIG_ENV" ] && [ "$TABSH_ORIG_ENV" != "$ENV" ] && [ -f "$TABSH_ORIG_ENV" ]; then
  . "$TABSH_ORIG_ENV"
fi
if [ -n "$TABSH_SHIM_DIR" ]; then
  case ":$PATH:" in
    *":$TABSH_SHIM_DIR:"*) ;;
    *) export PATH="$TABSH_SHIM_DIR:$PATH" ;;
  esac
fi
