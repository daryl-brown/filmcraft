# Run a native command, keep its full output, and on failure surface the useful part as ONE
# public annotation (readable through the check-runs annotations API without signing in).
#
# Usage (dot-source, then call):
#   . ./.github/scripts/ci-logged.ps1
#   Invoke-Logged -Title 'cargo test h264' -Command { cargo test --locked -p filmcraft-h264 }

function Invoke-Logged {
  param(
    [Parameter(Mandatory)][string]$Title,
    [Parameter(Mandatory)][scriptblock]$Command
  )

  $ErrorActionPreference = 'Continue'
  $dir = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [IO.Path]::GetTempPath() }
  $log = Join-Path $dir (($Title -replace '[^A-Za-z0-9]+', '-') + '.log')

  & $Command 2>&1 | ForEach-Object { "$_" } | Tee-Object -FilePath $log
  $code = $LASTEXITCODE
  if ($code -eq 0) { return }

  $lines = @(Get-Content -LiteralPath $log)
  $pattern = '^\s*error(\[|:)|^error: |panicked|FAILED|^failures:|^---- |mismatch|assertion'
  $keep = New-Object 'System.Collections.Generic.SortedSet[int]'
  for ($i = 0; $i -lt $lines.Count; $i++) {
    if ($lines[$i] -match $pattern) {
      for ($j = [Math]::Max(0, $i - 1); $j -le [Math]::Min($lines.Count - 1, $i + 4); $j++) { [void]$keep.Add($j) }
    }
  }
  $picked = @($keep | Select-Object -First 150 | ForEach-Object { $lines[$_] })
  $tail = @($lines | Select-Object -Last 40)
  $text = "exit code $code`n--- matched lines ---`n" + ($picked -join "`n") + "`n--- last 40 lines ---`n" + ($tail -join "`n")
  if ($text.Length -gt 40000) { $text = $text.Substring(0, 40000) + "`n[truncated]" }

  # Workflow command escaping: % CR LF.
  $enc = $text.Replace('%', '%25').Replace("`r", '%0D').Replace("`n", '%0A')
  Write-Output "::error title=$Title::$enc"
  throw "$Title failed with exit code $code"
}
