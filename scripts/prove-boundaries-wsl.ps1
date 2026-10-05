$ErrorActionPreference = 'Stop'

$script = (Resolve-Path (Join-Path $PSScriptRoot 'prove-boundaries.sh')).Path
$drive = $script.Substring(0, 1).ToLowerInvariant()
$rest = $script.Substring(2).Replace('\', '/')
$wslScript = "/mnt/$drive$rest"
wsl.exe -d Ubuntu-24.04 -u stephensookra -- bash $wslScript
if ($LASTEXITCODE -ne 0) {
    throw "WSL OpenShell boundary proof failed with exit $LASTEXITCODE"
}
