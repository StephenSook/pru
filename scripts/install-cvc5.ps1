$ErrorActionPreference = 'Stop'

$version = '1.3.1'
$archive = 'cvc5-Win64-x86_64-static.zip'
$expected = 'e1e183f50f1bbd9d10b2dc6ab9574cb9fa02acfdc1957f43040db3529f92b94e'
$destination = if ($env:CVC5_INSTALL_DIR) { $env:CVC5_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "Pru\cvc5-$version" }
$download = Join-Path ([System.IO.Path]::GetTempPath()) $archive
$url = "https://github.com/cvc5/cvc5/releases/download/cvc5-$version/$archive"

New-Item -ItemType Directory -Force $destination | Out-Null
Invoke-WebRequest -Uri $url -OutFile $download
$actual = (Get-FileHash -Algorithm SHA256 $download).Hash.ToLowerInvariant()
if ($actual -ne $expected) {
    throw "cvc5 checksum mismatch: expected=$expected actual=$actual"
}
Expand-Archive -LiteralPath $download -DestinationPath $destination -Force
$binary = Get-ChildItem -LiteralPath $destination -Filter cvc5.exe -File -Recurse | Select-Object -First 1
if (-not $binary) {
    throw "cvc5.exe not found under $destination"
}
& $binary.FullName --version
$binary.FullName
