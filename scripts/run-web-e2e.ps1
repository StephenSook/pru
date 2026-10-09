param(
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]] $PlaywrightArgs
)

$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
$runDir = Join-Path $env:TEMP ("pru-e2e-" + [guid]::NewGuid().ToString('N'))
$dataDir = Join-Path $runDir 'data'
New-Item -ItemType Directory -Path $dataDir -Force | Out-Null

$processes = [System.Collections.Generic.List[System.Diagnostics.Process]]::new()
$testExitCode = 0

foreach ($port in 3000, 8082, 8787) {
    if (Get-NetTCPConnection -State Listen -LocalPort $port -ErrorAction SilentlyContinue) {
        throw "Port $port is already occupied. Stop the existing service before this isolated run."
    }
}

function Start-HiddenProcess {
    param(
        [string] $FilePath,
        [string[]] $ArgumentList,
        [string] $WorkingDirectory,
        [hashtable] $Environment,
        [string] $LogName
    )

    $process = Start-Process `
        -FilePath $FilePath `
        -ArgumentList $ArgumentList `
        -WorkingDirectory $WorkingDirectory `
        -Environment $Environment `
        -WindowStyle Hidden `
        -RedirectStandardOutput (Join-Path $runDir "$LogName.log") `
        -RedirectStandardError (Join-Path $runDir "$LogName.err") `
        -PassThru
    $processes.Add($process)
    return $process
}

function Wait-Ready {
    param(
        [string] $Name,
        [string] $Url,
        [System.Diagnostics.Process] $Process
    )

    for ($attempt = 0; $attempt -lt 60; $attempt++) {
        if ($Process.HasExited) {
            throw "$Name stopped before readiness. Logs: $runDir"
        }
        try {
            Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 2 | Out-Null
            return
        } catch {
            Start-Sleep -Seconds 1
        }
    }
    throw "$Name did not become ready within 60 seconds. Logs: $runDir"
}

try {
    $node = (Get-Command node.exe).Source
    $corepack = (Get-Command corepack.cmd).Source
    $mock = Start-HiddenProcess `
        -FilePath $node `
        -ArgumentList @('scripts/mock-openai.mjs') `
        -WorkingDirectory $repoRoot `
        -Environment @{} `
        -LogName 'mock'

    $gateway = Start-HiddenProcess `
        -FilePath (Join-Path $repoRoot 'target\debug\pru-gateway.exe') `
        -ArgumentList @() `
        -WorkingDirectory $repoRoot `
        -Environment @{
            PRU_BIND = '127.0.0.1:8787'
            PRU_DATA_DIR = $dataDir
            PRU_LEDGER_HASH_KEY = ('1' * 64)
            PRU_LOCAL_LLM_URL = 'http://127.0.0.1:8082/v1'
            PRU_LOCAL_LLM_HEALTH_URL = 'http://127.0.0.1:8082/health'
            PRU_TOKEN_FACTORY_URL = 'http://127.0.0.1:8082/v1'
            NEBIUS_API_KEY = 'ci-test-key'
        } `
        -LogName 'gateway'

    $web = Start-HiddenProcess `
        -FilePath $node `
        -ArgumentList @('node_modules/next/dist/bin/next', 'start') `
        -WorkingDirectory (Join-Path $repoRoot 'web') `
        -Environment @{ PRU_GATEWAY_URL = 'http://127.0.0.1:8787' } `
        -LogName 'web'

    Wait-Ready -Name 'mock OpenAI server' -Url 'http://127.0.0.1:8082/health' -Process $mock
    Wait-Ready -Name 'gateway' -Url 'http://127.0.0.1:8787/healthz' -Process $gateway
    Wait-Ready -Name 'web' -Url 'http://127.0.0.1:3000/healthz' -Process $web

    Push-Location (Join-Path $repoRoot 'web')
    try {
        & $corepack pnpm e2e @PlaywrightArgs
        $testExitCode = $LASTEXITCODE
    } finally {
        Pop-Location
    }
} finally {
    foreach ($process in $processes) {
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id -Force -PassThru | Wait-Process -Timeout 5
        }
    }
}

if ($testExitCode -ne 0) {
    exit $testExitCode
}
