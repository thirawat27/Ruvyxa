<#
.SYNOPSIS
    Full integration test: every CLI command + build targets + error scenarios.
.DESCRIPTION
    Exercises the whole `ruvyxa` CLI surface against examples/demo: the happy
    path for each subcommand, production/dev server behavior for every render
    strategy, build target and deploy adapter output, and the build-time
    diagnostics (RUV*) raised by malformed apps.

    Run from the monorepo root after:
        pnpm install
        pnpm -r build
        cargo build -p ruvyxa_cli
.PARAMETER SkipAdapters
    Skip the build target / deploy adapter matrix (the slowest section).
#>

[CmdletBinding()]
param(
    [switch]$SkipAdapters
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$Ruvyxa = Join-Path $RepoRoot "target\debug\ruvyxa.exe"
$App = Join-Path $RepoRoot "examples\demo"

if (-not (Test-Path $Ruvyxa)) {
    throw "CLI binary not found at $Ruvyxa. Run 'cargo build -p ruvyxa_cli' first."
}

# Ports are allocated from one block so a partial run never collides with the
# next one, and so every listener used here is easy to find.
$Ports = @{
    Api      = 3991
    Ssg      = 3992
    Prod     = 3993
    Preview  = 3994
    Dev      = 3995
    DevMiss  = 3996
    NoBuild  = 3997
    DevCsr   = 3998
    DevSsg   = 3999
    DevIsr   = 4000
    DevCache = 4001
}

$script:Failures = @()
$script:Warnings = @()

function Write-Ok      { param([string]$Message) Write-Host "[OK] $Message"   -ForegroundColor Green }
function Write-Note    { param([string]$Message) Write-Host "[NOTE] $Message" -ForegroundColor DarkYellow }
function Write-Section { param([string]$Title)   Write-Host ""; Write-Host "=== $Title ===" -ForegroundColor Cyan; Write-Host "" }

function Write-Warn {
    param([string]$Message)
    $script:Warnings += $Message
    Write-Host "[WARN] $Message" -ForegroundColor DarkYellow
}

# Record a failure and keep going. A single broken scenario should not hide the
# state of every scenario after it; the run still exits non-zero at the end.
function Write-Fail {
    param([string]$Message)
    $script:Failures += $Message
    Write-Host "[FAIL] $Message" -ForegroundColor Red
}

function Stop-LeftoverServers {
    Get-Process -Name "ruvyxa" -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
}

function Invoke-Native {
    param([string[]]$Arguments)
    $previousPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        & $Ruvyxa @Arguments 2>&1
        $script:LastNativeExitCode = $LASTEXITCODE
    } finally {
        $ErrorActionPreference = $previousPreference
    }
}

# Run a subcommand against the demo app and require a zero exit code.
function Test-Cli {
    param(
        [string]$Description,
        [string[]]$Arguments,
        [switch]$AllowFailure
    )
    Write-Host "--- $Description ---" -ForegroundColor Yellow
    Invoke-Native -Arguments ($Arguments + @("--root", $App)) | Out-Host
    if ($script:LastNativeExitCode -eq 0) {
        Write-Ok $Description
    } elseif ($AllowFailure) {
        Write-Warn "$Description exit $($script:LastNativeExitCode)"
    } else {
        Write-Fail "$Description exit $($script:LastNativeExitCode)"
    }
    Write-Host ""
}

# Start a server and wait until it actually answers, instead of sleeping a fixed
# number of seconds. Fixed sleeps are the main source of flakiness here: they are
# simultaneously too short on a cold cache and wasted time on a warm one.
function Start-RuvyxaServer {
    param(
        [ValidateSet("dev", "start", "preview")][string]$Mode,
        [int]$Port,
        [string]$Root = $App,
        [int]$TimeoutSeconds = 45
    )
    $log = Join-Path $env:TEMP "ruvyxa-$Mode-$Port.log"
    Remove-Item $log -Force -ErrorAction SilentlyContinue
    $process = Start-Process -NoNewWindow -FilePath $Ruvyxa `
        -ArgumentList @($Mode, "--root", $Root, "--port", $Port) `
        -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        if ($process.HasExited) {
            throw "$Mode server on port $Port exited early (code $($process.ExitCode)). Log: $log"
        }
        try {
            # Any HTTP answer -- including 404 -- proves the listener is up.
            Invoke-WebRequest -Uri "http://localhost:$Port/" -UseBasicParsing -TimeoutSec 3 | Out-Null
            return [pscustomobject]@{ Process = $process; Port = $Port; Log = $log }
        } catch [System.Net.WebException], [Microsoft.PowerShell.Commands.HttpResponseException] {
            if ($_.Exception.Response) {
                return [pscustomobject]@{ Process = $process; Port = $Port; Log = $log }
            }
        } catch {
            # Connection refused while the server is still binding.
        }
        Start-Sleep -Milliseconds 250
    }
    $process | Stop-Process -Force -ErrorAction SilentlyContinue
    throw "$Mode server on port $Port did not become ready in ${TimeoutSeconds}s. Log: $log"
}

function Stop-RuvyxaServer {
    param($Server)
    if ($Server -and $Server.Process -and -not $Server.Process.HasExited) {
        $Server.Process | Stop-Process -Force -ErrorAction SilentlyContinue
    }
}

function Invoke-App {
    param([int]$Port, [string]$Path, [string]$Method = "GET", $Body, [string]$ContentType)
    $arguments = @{
        Uri             = "http://localhost:$Port$Path"
        Method          = $Method
        UseBasicParsing = $true
        TimeoutSec      = 15
    }
    if ($null -ne $Body) { $arguments.Body = $Body }
    if ($ContentType) { $arguments.ContentType = $ContentType }
    return Invoke-WebRequest @arguments
}

# Replace a file for the duration of a scenario, then always restore it. Doing
# this by hand is what leaves the demo app dirty when an assertion throws.
function Use-TemporaryContent {
    param([string]$Path, [string]$Content, [scriptblock]$Body)
    $original = Get-Content -LiteralPath $Path -Raw
    try {
        Set-Content -LiteralPath $Path -Value $Content -Force -NoNewline
        & $Body
    } finally {
        Set-Content -LiteralPath $Path -Value $original -Force -NoNewline
    }
}

# Same idea for scratch route directories: create, run, always remove.
function Use-TemporaryRoute {
    param([hashtable]$Files, [string]$Root, [scriptblock]$Body)
    $absoluteRoot = Join-Path $App $Root
    try {
        foreach ($relative in $Files.Keys) {
            $target = Join-Path $absoluteRoot $relative
            [System.IO.Directory]::CreateDirectory((Split-Path -Parent $target)) | Out-Null
            Set-Content -LiteralPath $target -Value $Files[$relative] -Force
        }
        & $Body
    } finally {
        Remove-Item -LiteralPath $absoluteRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# Assert that `analyze` reports a specific diagnostic code.
function Test-Diagnostic {
    param([string]$Description, [string]$Code, [switch]$Optional)
    $output = Invoke-Native -Arguments @("analyze", "--root", $App) | Out-String
    if ($output -match $Code) {
        Write-Ok "$Description -> $Code"
    } elseif ($Optional) {
        Write-Warn "$Description -> $Code not reported"
    } else {
        Write-Fail "$Description -> $Code not reported"
    }
}

# ==============================================================================
#  WELCOME
# ==============================================================================
Write-Host "===============================================" -ForegroundColor Cyan
Write-Host "    Ruvyxa Full Integration Test Suite" -ForegroundColor Cyan
Write-Host "===============================================" -ForegroundColor Cyan
Write-Host "CLI:            $Ruvyxa" -ForegroundColor Gray
Write-Host "App under test: $App" -ForegroundColor Gray

Stop-LeftoverServers
Start-Sleep -Seconds 1

# Clean up leftover scratch routes from previously interrupted runs.
@(
    "app/full-flow-lib",
    "app/full-flow-bad-segment",
    "app/full-flow-conflict",
    "app/full-flow-dyn-ssg",
    "app/full-flow-bad-params",
    "app/full-flow-conflict-strat",
    "app/bad-page"
) | ForEach-Object {
    $path = Join-Path $App $_
    if (Test-Path -LiteralPath $path) {
        Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue
    }
}

try {

# ==============================================================================
#  1. SCAFFOLDING
# ==============================================================================
Write-Section "SCAFFOLDING"

$CreateDist = Join-Path $RepoRoot "packages\create-ruvyxa\dist\index.js"
if (Test-Path $CreateDist) {
    $CreateRoot = Join-Path $env:TEMP "ruvyxa-create-demo-$(Get-Random)"
    try {
        New-Item -ItemType Directory -Path $CreateRoot -Force | Out-Null
        node (Join-Path $RepoRoot "packages\create-ruvyxa\bin\create-ruvyxa.js") (Join-Path $CreateRoot "demo-app") | Out-Host
        Write-Ok "create-ruvyxa generates a project"
    } finally {
        Remove-Item $CreateRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
} else {
    Write-Warn "create-ruvyxa dist not built (run 'pnpm -r build' first)"
}

# `plugin new` scaffolds into --root, so keep it out of the demo app.
$PluginRoot = Join-Path $env:TEMP "ruvyxa-plugin-$(Get-Random)"
try {
    New-Item -ItemType Directory -Path $PluginRoot -Force | Out-Null
    Invoke-Native -Arguments @("plugin", "new", "ruvyxa-plugin-smoke", "--root", $PluginRoot) | Out-Host
    $pluginManifest = Join-Path $PluginRoot "ruvyxa-plugin-smoke\package.json"
    if ($script:LastNativeExitCode -eq 0 -and (Test-Path $pluginManifest)) {
        Write-Ok "plugin new scaffolds a publishable package"
    } else {
        Write-Fail "plugin new (exit $($script:LastNativeExitCode))"
    }

    # --dir must place the package somewhere other than <name>.
    Invoke-Native -Arguments @("plugin", "new", "ruvyxa-plugin-dir", "--root", $PluginRoot, "--dir", "custom-dir") | Out-Host
    if ($script:LastNativeExitCode -eq 0 -and (Test-Path (Join-Path $PluginRoot "custom-dir\package.json"))) {
        Write-Ok "plugin new --dir honors a custom directory"
    } else {
        Write-Fail "plugin new --dir (exit $($script:LastNativeExitCode))"
    }
} finally {
    Remove-Item $PluginRoot -Recurse -Force -ErrorAction SilentlyContinue
}

# ==============================================================================
#  2. CLI COMMANDS - happy path
# ==============================================================================
Write-Section "CLI COMMANDS (happy path)"

Test-Cli "analyze" @("analyze")
Test-Cli "routes"  @("routes")
# `check` shells out to tsc, so it only fails meaningfully once node_modules exist.
Test-Cli "check"   @("check") -AllowFailure
Test-Cli "doctor"  @("doctor")
Test-Cli "trace /" @("trace", "/")
Test-Cli "bench (1 sample)" @("bench", "--samples", "1") -AllowFailure

# ==============================================================================
#  3. BUILD + OUTPUT VERIFICATION
# ==============================================================================
Write-Section "BUILD"

Test-Cli "build" @("build")

$OutDir       = Join-Path $App ".ruvyxa"
$ClientDir    = Join-Path $OutDir "client"
$PrerenderDir = Join-Path $OutDir "prerender"

foreach ($name in @("server", "client", "assets", "prerender")) {
    if (Test-Path (Join-Path $OutDir $name)) {
        Write-Ok "build: .ruvyxa/$name exists"
    } else {
        Write-Fail "build: .ruvyxa/$name missing"
    }
}

$ClientManifestPath = Join-Path $ClientDir "manifest.json"
if (Test-Path $ClientManifestPath) {
    # The client manifest is machine-read on the SSR path, so assert it parses
    # and carries per-route entries rather than only that the file exists.
    $clientManifest = Get-Content $ClientManifestPath -Raw | ConvertFrom-Json
    if ($clientManifest.routes.Count -gt 0) {
        Write-Ok "build: client/manifest.json ($($clientManifest.routes.Count) routes)"
    } else {
        Write-Fail "build: client/manifest.json has no routes"
    }
} else {
    Write-Fail "build: client/manifest.json missing"
}

$BuildJson = Join-Path $OutDir "build.json"
if (Test-Path $BuildJson) {
    $buildInfo = Get-Content $BuildJson -Raw | ConvertFrom-Json
    Write-Ok "build: build.json (routes=$($buildInfo.routes), prerendered=$($buildInfo.rendering.prerendered))"
} else {
    Write-Fail "build: build.json missing"
}

$ManifestJson = Join-Path $OutDir "manifest.json"
if (Test-Path $ManifestJson) {
    $manifest = Get-Content $ManifestJson -Raw | ConvertFrom-Json
    $pagesWithStrategies = @($manifest.routes | Where-Object { $_.kind -eq "page" -and $null -ne $_.render })
    if ($pagesWithStrategies.Count -gt 0) {
        Write-Ok "manifest: $($pagesWithStrategies.Count) page routes carry a render strategy"
    } else {
        Write-Fail "manifest: no page route records a render strategy"
    }
} else {
    Write-Fail "manifest.json missing"
}

$ClientBundleCount = @(Get-ChildItem $ClientDir -Filter "*.js" -ErrorAction SilentlyContinue).Count
if ($ClientBundleCount -gt 0) {
    Write-Ok "build: $ClientBundleCount client bundles emitted"
} else {
    Write-Fail "build: no client bundles found"
}

# --- PRERENDER OUTPUT -----------------------------------------------------------
Write-Host "--- prerendered route verification ---" -ForegroundColor Yellow

$PrerenderManifest = Join-Path $PrerenderDir "manifest.json"
if (Test-Path $PrerenderManifest) {
    $prerenderInfo = Get-Content $PrerenderManifest -Raw | ConvertFrom-Json
    Write-Ok "prerender: manifest.json ($($prerenderInfo.routes.Count) routes)"
} else {
    Write-Warn "prerender: manifest.json not found (SSG renderer may be unavailable)"
}

$SsgHtml = Join-Path $PrerenderDir "ssg-blog/hello-world/index.html"
if (Test-Path $SsgHtml) {
    if ((Get-Content $SsgHtml -Raw) -match "<!doctype html>") {
        Write-Ok "prerender: /ssg-blog/hello-world -> valid HTML"
    } else {
        Write-Warn "prerender: /ssg-blog/hello-world HTML looks incomplete"
    }
} else {
    Write-Warn "prerender: /ssg-blog/hello-world not found"
}

$CsrHtml = Join-Path $PrerenderDir "csr-page/index.html"
if (Test-Path $CsrHtml) {
    if ((Get-Content $CsrHtml -Raw) -match "<div id=.__ruvyxa.>") {
        Write-Ok "prerender: /csr-page -> CSR shell (no SSR content)"
    } else {
        Write-Warn "prerender: /csr-page shell looks off"
    }
} else {
    Write-Warn "prerender: /csr-page not found"
}

foreach ($entry in @(@{ Route = "isr-page"; Label = "ISR HTML" }, @{ Route = "ppr-page"; Label = "PPR shell" })) {
    if (Test-Path (Join-Path $PrerenderDir "$($entry.Route)/index.html")) {
        Write-Ok "prerender: /$($entry.Route) -> $($entry.Label)"
    } else {
        Write-Warn "prerender: /$($entry.Route) not found"
    }
}

# SSR routes must stay out of the prerender output. The route list is derived
# from the build manifest rather than hard-coded: which demo pages are SSR
# changes as the demo app evolves (a page that stops reading request state is
# promoted to SSG), and a hard-coded list silently stops testing anything the
# moment it drifts.
if ($manifest) {
    $ssrRoutes = @($manifest.routes | Where-Object { $_.kind -eq "page" -and $_.render.strategy -eq "ssr" })
    $ssrLeaked = @($ssrRoutes | Where-Object {
        # Dynamic segments never produce a literal output path, so only concrete
        # SSR routes can leak a prerendered file.
        $_.path -notmatch '\[' -and
        (Test-Path (Join-Path $PrerenderDir ($_.path.Trim('/') + "/index.html")))
    })
    if ($ssrRoutes.Count -eq 0) {
        Write-Note "prerender: demo app currently has no static SSR page routes to check"
    } elseif ($ssrLeaked.Count -eq 0) {
        Write-Ok "prerender: all $($ssrRoutes.Count) SSR routes correctly skipped"
    } else {
        Write-Fail "prerender: SSR routes were pre-rendered: $(($ssrLeaked.path) -join ', ')"
    }

    # The inverse must hold too, otherwise a build that pre-renders nothing at
    # all would pass the check above.
    $ssgRoutes = @($manifest.routes | Where-Object {
        $_.kind -eq "page" -and $_.render.strategy -eq "ssg" -and
        $_.path -notmatch '\[' -and $_.path -ne "/"
    })
    $ssgMissing = @($ssgRoutes | Where-Object {
        -not (Test-Path (Join-Path $PrerenderDir ($_.path.Trim('/') + "/index.html")))
    })
    if ($ssgMissing.Count -eq 0) {
        Write-Ok "prerender: all $($ssgRoutes.Count) SSG routes emitted static HTML"
    } else {
        Write-Fail "prerender: SSG routes missing static HTML: $(($ssgMissing.path) -join ', ')"
    }
}

# ==============================================================================
#  4. BUILD TARGETS + DEPLOY ADAPTERS
# ==============================================================================
if ($SkipAdapters) {
    Write-Section "BUILD TARGETS / ADAPTERS (skipped)"
} else {
    Write-Section "BUILD TARGETS / ADAPTERS"

    # `--target` selects the server runtime the bundle is emitted for; `edge`
    # and `static` in particular have their own code paths and must not regress.
    foreach ($target in @("node", "bun", "edge", "static")) {
        Test-Cli "build --target $target" @("build", "--target", $target)
    }

    # `--adapter` picks the deploy shape without editing ruvyxa.config. Each
    # adapter declares the strategies it can deploy via `Adapter.supports`, and
    # the runner rejects routes outside that set with RUV2202 before building.
    # The demo app exercises every strategy, so an adapter that cannot host all
    # of them is *expected* to fail -- asserting the expected outcome catches
    # both a regression that breaks a working adapter and one that silently
    # stops enforcing a platform limit.
    $adapterExpectations = @(
        @{ Name = "node";       Supported = $true }
        @{ Name = "bun";        Supported = $true }
        @{ Name = "vercel";     Supported = $true }
        @{ Name = "netlify";    Supported = $true }
        # No writable prerender cache on a Worker asset binding, so no ISR/PPR.
        @{ Name = "cloudflare"; Supported = $false; Unsupported = @("/isr-page", "/ppr-page") }
        # A static publish directory has no server at all.
        @{ Name = "static";     Supported = $false; Unsupported = @("/api/health", "/blog/[slug]", "/isr-page") }
    )
    foreach ($expectation in $adapterExpectations) {
        $label = "build --adapter $($expectation.Name)"
        Write-Host "--- $label ---" -ForegroundColor Yellow
        $output = Invoke-Native -Arguments @("build", "--adapter", $expectation.Name, "--root", $App) | Out-String
        $exit = $script:LastNativeExitCode

        if ($expectation.Supported) {
            if ($exit -eq 0) { Write-Ok $label }
            else { Write-Fail "$label exit $exit`n$output" }
        } elseif ($exit -eq 0) {
            Write-Fail "$label unexpectedly succeeded; RUV2202 should reject $($expectation.Unsupported -join ', ')"
        } elseif ($output -notmatch "RUV2202") {
            Write-Fail "$label failed without RUV2202`n$output"
        } else {
            $missing = @($expectation.Unsupported | Where-Object { $output -notmatch [regex]::Escape($_) })
            if ($missing.Count -eq 0) {
                Write-Ok "$label -> RUV2202 names every unsupported route"
            } else {
                Write-Fail "$label RUV2202 omitted: $($missing -join ', ')"
            }
        }
        Write-Host ""
    }

    # Leave the demo app on the default build so later sections test the real
    # production server rather than an adapter artifact.
    Test-Cli "build (restore default output)" @("build")
}

# ==============================================================================
#  5. PARITY
# ==============================================================================
Write-Section "DEV/PROD PARITY"

Test-Cli "test:parity" @("test:parity") -AllowFailure

# ==============================================================================
#  6. PRODUCTION SERVER
# ==============================================================================
Write-Section "API ROUTES"

$apiServer = Start-RuvyxaServer -Mode "start" -Port $Ports.Api
try {
    $health = Invoke-App -Port $Ports.Api -Path "/api/health"
    if ($health.StatusCode -eq 200) {
        Write-Ok "api: GET /api/health -> 200 (status=$(($health.Content | ConvertFrom-Json).status))"
    } else {
        Write-Fail "api: /api/health returned $($health.StatusCode)"
    }

    # Request bodies are forwarded to the worker pool as base64 frames, so a
    # POST round-trip is a hard requirement rather than a known gap.
    $echo = Invoke-App -Port $Ports.Api -Path "/api/echo" -Method POST `
        -Body (@{ message = "hello" } | ConvertTo-Json) -ContentType "application/json"
    # The demo route echoes the parsed request body back under `body`, alongside
    # the method and path it observed.
    $echoBody = $echo.Content | ConvertFrom-Json
    if ($echo.StatusCode -eq 200 -and $echoBody.body.message -eq "hello" -and $echoBody.method -eq "POST") {
        Write-Ok "api: POST /api/echo -> 200 (body round-trips)"
    } else {
        Write-Fail "api: POST /api/echo returned $($echo.StatusCode) body=$($echo.Content)"
    }
} catch {
    Write-Fail "api server: $_"
} finally {
    Stop-RuvyxaServer $apiServer
}

Write-Section "PRODUCTION SERVER (render strategies)"

$ssgServer = Start-RuvyxaServer -Mode "start" -Port $Ports.Ssg
try {
    $strategyRoutes = @(
        @{ Path = "/ssg-blog/hello-world"; Label = "SSG page";        RequireHtml = $true }
        @{ Path = "/csr-page";             Label = "CSR shell";       RequireHtml = $true }
        @{ Path = "/isr-page";             Label = "ISR page";        RequireHtml = $false }
        @{ Path = "/ppr-page";             Label = "PPR shell";       RequireHtml = $false }
        @{ Path = "/about";                Label = "SSR page";        RequireHtml = $true }
        @{ Path = "/ssg-blog";             Label = "SSG index (SSR)"; RequireHtml = $false }
        @{ Path = "/static-page";          Label = "static page";     RequireHtml = $false }
    )
    foreach ($route in $strategyRoutes) {
        try {
            $response = Invoke-App -Port $Ports.Ssg -Path $route.Path
            if ($response.StatusCode -ne 200) {
                Write-Fail "prod: $($route.Path) returned $($response.StatusCode)"
            } elseif ($route.RequireHtml -and $response.Content -notmatch "<!doctype html>") {
                Write-Fail "prod: $($route.Path) ($($route.Label)) returned incomplete HTML"
            } else {
                Write-Ok "prod: GET $($route.Path) -> 200 ($($route.Label))"
            }
        } catch {
            Write-Fail "prod: $($route.Path) -> $_"
        }
    }
} finally {
    Stop-RuvyxaServer $ssgServer
}

Write-Section "START / PREVIEW"

$prodServer = Start-RuvyxaServer -Mode "start" -Port $Ports.Prod
try {
    $response = Invoke-App -Port $Ports.Prod -Path "/"
    if ($response.StatusCode -eq 200) { Write-Ok "start: production server responds 200" }
    else { Write-Fail "start: production server returned $($response.StatusCode)" }
} catch {
    Write-Fail "start server: $_"
    Get-Content $prodServer.Log -Tail 5 -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "  log: $_" }
} finally {
    Stop-RuvyxaServer $prodServer
}

$previewServer = Start-RuvyxaServer -Mode "preview" -Port $Ports.Preview
try {
    $response = Invoke-App -Port $Ports.Preview -Path "/"
    if ($response.StatusCode -eq 200) { Write-Ok "preview: serves the existing production build" }
    else { Write-Fail "preview: returned $($response.StatusCode)" }
} catch {
    Write-Fail "preview server: $_"
} finally {
    Stop-RuvyxaServer $previewServer
}

# ==============================================================================
#  7. CLIENT MANIFEST CACHE INVALIDATION
# ==============================================================================
Write-Section "CLIENT MANIFEST CACHE"

# The SSR path caches the parsed client manifest. Because a rebuild usually only
# changes the content hash inside each bundle URL, the rewritten manifest can be
# byte-for-byte the same length as the previous one -- so the cache must key on
# content, not on (mtime, length). Serving a stale bundle URL here means the
# browser 404s on hydration.
#
# Two constraints shape this probe:
#   * It must use an SSR route. SSG/ISR/PPR routes are answered from the
#     pre-rendered HTML on disk and never consult the client manifest at request
#     time, so rewriting the manifest could not change their output.
#   * The two requests must use *different* URLs. The production render cache
#     stores whole documents keyed by URL with an effectively infinite TTL, so
#     re-requesting the same path replays the cached HTML and the manifest is
#     never re-read -- the check would report a stale bundle no matter what.
$CacheProbeBefore = "/catchall/manifest-cache-probe-a"
$CacheProbeAfter  = "/catchall/manifest-cache-probe-b"
$cacheServer = Start-RuvyxaServer -Mode "start" -Port $Ports.DevCache
try {
    $originalManifest = Get-Content -LiteralPath $ClientManifestPath -Raw
    $before = Invoke-App -Port $Ports.DevCache -Path $CacheProbeBefore
    $beforeSrc = ([regex]::Match($before.Content, 'src="(/__ruvyxa/client/[^"]+)"')).Groups[1].Value

    if (-not $beforeSrc) {
        Write-Warn "manifest cache: no client bundle referenced on $CacheProbeBefore (skipping)"
    } else {
        # Rewrite one bundle URL to an equal-length replacement, mimicking a
        # rebuild that only changed a content hash.
        $rewrittenSrc = $beforeSrc -replace '[0-9a-f](?=\.(js|mjs)$)', 'z'
        if ($rewrittenSrc -eq $beforeSrc) {
            Write-Warn "manifest cache: bundle URL has no hash suffix to perturb (skipping)"
        } else {
            try {
                Set-Content -LiteralPath $ClientManifestPath `
                    -Value $originalManifest.Replace($beforeSrc, $rewrittenSrc) -Force -NoNewline
                $after = Invoke-App -Port $Ports.DevCache -Path $CacheProbeAfter
                if ($after.Content -match [regex]::Escape($rewrittenSrc)) {
                    Write-Ok "manifest cache: same-length rewrite invalidates the cached parse"
                } else {
                    Write-Fail "manifest cache: served stale bundle URL after a same-length rewrite"
                }
            } finally {
                Set-Content -LiteralPath $ClientManifestPath -Value $originalManifest -Force -NoNewline
            }
        }
    }
} catch {
    Write-Fail "manifest cache: $_"
} finally {
    Stop-RuvyxaServer $cacheServer
}

# ==============================================================================
#  8. DEV SERVER
# ==============================================================================
Write-Section "DEV SERVER"

$devServer = Start-RuvyxaServer -Mode "dev" -Port $Ports.Dev
try {
    $response = Invoke-App -Port $Ports.Dev -Path "/"
    if ($response.StatusCode -eq 200) { Write-Ok "dev: normal page returns 200" }
    else { Write-Fail "dev: / returned $($response.StatusCode)" }

    # Dev-mode render strategies share one server; no reason to pay a cold start
    # for each of them.
    foreach ($route in @(
        @{ Path = "/csr-page";             Label = "CSR page shell"; Pattern = "<div id=.__ruvyxa.>" }
        @{ Path = "/ssg-blog/hello-world"; Label = "SSG page";       Pattern = "<!doctype html>" }
        @{ Path = "/isr-page";             Label = "ISR page";       Pattern = $null }
        @{ Path = "/ppr-page";             Label = "PPR page";       Pattern = $null }
    )) {
        try {
            $r = Invoke-App -Port $Ports.Dev -Path $route.Path
            if ($r.StatusCode -ne 200) {
                Write-Fail "dev: $($route.Path) returned $($r.StatusCode)"
            } elseif ($route.Pattern -and $r.Content -notmatch $route.Pattern) {
                Write-Fail "dev: $($route.Label) content unexpected"
            } else {
                Write-Ok "dev: $($route.Label) renders"
            }
        } catch {
            Write-Fail "dev: $($route.Path) -> $_"
        }
    }

    # An unknown route must not 200.
    try {
        $missing = Invoke-App -Port $Ports.Dev -Path "/nonexistent"
        Write-Fail "dev: unknown route returned $($missing.StatusCode), expected 404"
    } catch {
        $status = $_.Exception.Response.StatusCode.value__
        if ($status -eq 404) { Write-Ok "dev: unknown route returns 404" }
        else { Write-Warn "dev: unknown route returned $status" }
    }
} finally {
    Stop-RuvyxaServer $devServer
}

# ==============================================================================
#  9. BUILD-TIME ERROR SCENARIOS
# ==============================================================================
Write-Section "BUILD-TIME ERROR SCENARIOS"

# E1: page without a default export.
Use-TemporaryRoute -Root "app/bad-page" -Files @{
    "page.tsx" = "export function NotDefault() {`n  return null`n}"
} -Body {
    Test-Diagnostic "E1: missing default export" "RUV1004"
}

# E2: server-only module pulled into the client graph.
Use-TemporaryRoute -Root "app/full-flow-lib" -Files @{
    "db.ts" = "import `"server-only`"`nexport const db = {}"
} -Body {
    Use-TemporaryContent -Path (Join-Path $App "app\page.tsx") -Content @"
import { db } from "./full-flow-lib/db"
export default function Home() {
  return <main>{JSON.stringify(db)}</main>
}
"@ -Body {
        Test-Diagnostic "E2: server-only in client graph" "RUV1007"
    }
}

# E3: catch-all segment that is not in final position.
Use-TemporaryRoute -Root "app/full-flow-bad-segment" -Files @{
    "[...slug]/extra/page.tsx" = "export default function CatchAll() { return null }"
} -Body {
    Test-Diagnostic "E3: invalid route segment" "RUV1002"
}

# E4: two dynamic segments that resolve to the same route.
Use-TemporaryRoute -Root "app/full-flow-conflict" -Files @{
    "[slug]/page.tsx" = "export default function Post() { return <div>Post</div> }"
    "[post]/page.tsx" = "export default function Post2() { return <div>Post2</div> }"
} -Body {
    Test-Diagnostic "E4: conflicting routes" "RUV1003"
}

# E5: `start` against a project that was never built.
$NoBuildApp = Join-Path $env:TEMP "ruvyxa-nobuild-$(Get-Random)"
try {
    node (Join-Path $RepoRoot "packages\create-ruvyxa\bin\create-ruvyxa.js") $NoBuildApp | Out-Null
    $output = Invoke-Native -Arguments @("start", "--root", $NoBuildApp, "--port", $Ports.NoBuild) | Out-String
    if ($script:LastNativeExitCode -ne 0 -or $output -match "Error|not found|build") {
        Write-Ok "E5: start without build -> error"
    } else {
        Write-Fail "E5: start without build unexpectedly succeeded"
    }
} catch {
    Write-Warn "E5: could not scaffold a test project ($_)"
} finally {
    Remove-Item $NoBuildApp -Recurse -Force -ErrorAction SilentlyContinue
}

# E6: config with an empty required field.
Use-TemporaryContent -Path (Join-Path $App "ruvyxa.config.ts") -Content @"
import { config } from "ruvyxa/config"
export default config({ appDir: "", outDir: ".ruvyxa" })
"@ -Body {
    Test-Diagnostic "E6: invalid config" "RUV1601"
}

# E7: dynamic route without getStaticParams is valid SSR, so analyze must stay clean.
Use-TemporaryRoute -Root "app/full-flow-dyn-ssg" -Files @{
    "[id]/page.tsx" = @"
export default function DynamicSsg({ params }: { params: { id: string } }) {
  return <div>Dynamic {params.id}</div>
}
"@
} -Body {
    Invoke-Native -Arguments @("analyze", "--root", $App) | Out-Null
    if ($script:LastNativeExitCode -eq 0) {
        Write-Ok "E7: dynamic route without getStaticParams -> no error (valid SSR)"
    } else {
        Write-Fail "E7: dynamic route without getStaticParams failed analyze"
    }
}

# E8: getStaticParams with a non-array return. Not currently a hard error; the
# check exists so a future diagnostic is noticed rather than silently absorbed.
Use-TemporaryRoute -Root "app/full-flow-bad-params" -Files @{
    "[id]/page.tsx" = @"
export const getStaticParams = () => "invalid"
export default function BadParams() {
  return <div>Bad</div>
}
"@
} -Body {
    $output = Invoke-Native -Arguments @("analyze", "--root", $App) | Out-String
    if ($output -match "RUV\d{4}") {
        Write-Ok "E8: invalid getStaticParams -> diagnostic reported"
    } else {
        Write-Note "E8: invalid getStaticParams is not flagged by analyze (build-time only)"
    }
}

# E9: clean removes the output directory and a rebuild restores it.
Test-Cli "clean" @("clean")
if (Test-Path $OutDir) {
    Write-Fail "E9: .ruvyxa still exists after clean"
} else {
    Write-Ok "E9: clean removes .ruvyxa"
}
Test-Cli "E9: rebuild after clean" @("build")

# E10: ppr + revalidate on one page. `ppr` wins; the requirement is no crash.
Use-TemporaryRoute -Root "app/full-flow-conflict-strat" -Files @{
    "page.tsx" = @"
import { Suspense } from 'react'
export const ppr = true
export const revalidate = 30
export default function ConflictStrat() {
  return <div><Suspense fallback={<div>Loading...</div>}><div>Content</div></Suspense></div>
}
"@
} -Body {
    Invoke-Native -Arguments @("analyze", "--root", $App) | Out-Null
    if ($script:LastNativeExitCode -eq 0) {
        Write-Ok "E10: ppr + revalidate on one page -> no crash"
    } else {
        Write-Fail "E10: ppr + revalidate on one page failed analyze (exit $($script:LastNativeExitCode))"
    }
}

}
finally {
    Stop-LeftoverServers
}

# ==============================================================================
#  SUMMARY
# ==============================================================================
Write-Host ""
Write-Host "===============================================" -ForegroundColor Cyan
Write-Host "                   SUMMARY" -ForegroundColor Cyan
Write-Host "===============================================" -ForegroundColor Cyan

if ($script:Warnings.Count -gt 0) {
    Write-Host ""
    Write-Host "Warnings ($($script:Warnings.Count)):" -ForegroundColor DarkYellow
    $script:Warnings | ForEach-Object { Write-Host "  - $_" -ForegroundColor DarkYellow }
}

Write-Host ""
if ($script:Failures.Count -gt 0) {
    Write-Host "Failures ($($script:Failures.Count)):" -ForegroundColor Red
    $script:Failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    Write-Host ""
    exit 1
}

Write-Host "All integration checks passed." -ForegroundColor Green
Write-Host ""
exit 0
