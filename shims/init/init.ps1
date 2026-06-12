if ($env:TABSH_SHIM_DIR) {
  $env:PATH = $env:TABSH_SHIM_DIR + [IO.Path]::PathSeparator + $env:PATH
}

$global:__tabshPrompt = $function:prompt
function global:prompt {
  $e = [char]27
  $b = [char]7
  $p = $PWD.ProviderPath -replace '\\', '/'
  if (-not $p.StartsWith('/')) { $p = "/$p" }
  Write-Host -NoNewline "$e]7;file://$([Environment]::MachineName)$p$b$e]777;cmd;$b"
  & $global:__tabshPrompt
}
