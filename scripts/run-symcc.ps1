$ErrorActionPreference = 'Stop'

$defaultRoot = Join-Path $env:LOCALAPPDATA 'Pru\cvc5-1.3.1'
if (-not $env:CVC5) {
    $binary = Get-ChildItem -LiteralPath $defaultRoot -Filter cvc5.exe -File -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if (-not $binary) {
        & (Join-Path $PSScriptRoot 'install-cvc5.ps1') | Out-Host
        $binary = Get-ChildItem -LiteralPath $defaultRoot -Filter cvc5.exe -File -Recurse | Select-Object -First 1
    }
    $env:CVC5 = $binary.FullName
}

& $env:CVC5 --version
if ($LASTEXITCODE -ne 0) {
    throw "cvc5 failed with exit $LASTEXITCODE"
}
Push-Location (Join-Path $PSScriptRoot '..')
try {
    cargo test -p pru-policy --features symcc --test symcc -- --nocapture
    if ($LASTEXITCODE -ne 0) {
        throw "SymCC proof failed with exit $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}
