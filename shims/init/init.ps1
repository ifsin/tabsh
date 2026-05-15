# tabsh notification shim init
$profiles = @(
  $PROFILE.AllUsersAllHosts,
  $PROFILE.AllUsersCurrentHost,
  $PROFILE.CurrentUserAllHosts,
  $PROFILE.CurrentUserCurrentHost
)
foreach ($p in $profiles) {
  if (Test-Path $p) { . $p }
}
if ($env:TABSH_SHIM_DIR) {
  $env:PATH = "$env:TABSH_SHIM_DIR;$env:PATH"
}
