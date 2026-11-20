# feature ビルドマトリクス（W7）: noiria-app の 3 通りの feature 組み合わせが
# すべて `cargo build` で通ることを確認する。
#
# 使い方（PowerShell）:
#   .\scripts\feature-build-matrix.ps1
#
# 対象:
#   1. default                                        （engine-v8 + engine-tsint、開発の既定）
#   2. --no-default-features --features engine-v8       （V8 のみ、tsint 抜きでも壊れないことの確認）
#   3. --no-default-features --features engine-tsint,dist （配布ビルド本体。Linux 側は
#      scripts/build-dist-wsl.sh で別途 WSL 上でも確認すること）
#
# clippy は各構成のビルド後に `-- -D warnings` で実行し、警告が 1 件でもあれば（依存の noiria-core 等の警告を含む）その構成を失敗扱いにする。
#
# `cargo test` はここでは実行しない（default 以外は tag_behavior_tests 等
# 両エンジン前提のテストが `#[cfg(all(test, feature = "engine-v8", feature =
# "engine-tsint"))]` で自動的に対象外になるため、通常の `cargo test --workspace`
# は default 構成のみで検証すれば十分。ここではビルド可否だけを見る）。

$ErrorActionPreference = "Stop"
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$RepoRoot = Split-Path -Parent $PSScriptRoot
Set-Location $RepoRoot

$targets = @(
    @{ Name = "default (engine-v8 + engine-tsint)"; Args = @() },
    @{ Name = "engine-v8 only"; Args = @("--no-default-features", "--features", "engine-v8") },
    @{ Name = "engine-tsint + dist (配布ビルド)"; Args = @("--no-default-features", "--features", "engine-tsint,dist", "--profile", "dist") }
)

$results = @()
foreach ($t in $targets) {
    Write-Host ""
    Write-Host "=== $($t.Name) ===" -ForegroundColor Cyan
    $cargoArgs = @("build", "-p", "noiria-app") + $t.Args
    Write-Host "cargo $($cargoArgs -join ' ')"
    & cargo @cargoArgs
    $ok = ($LASTEXITCODE -eq 0)
    $clippyStatus = "-"
    if ($ok) {
        $clippyArgs = @("clippy", "-p", "noiria-app") + ($t.Args | Where-Object { $_ -ne "--profile" -and $_ -ne "dist" }) + @("--", "-D", "warnings")
        Write-Host "cargo $($clippyArgs -join ' ')"
        & cargo @clippyArgs
        if ($LASTEXITCODE -eq 0) {
            $clippyStatus = "0"
        } else {
            $clippyStatus = "あり"
            $ok = $false
        }
    }
    $results += [pscustomobject]@{ Target = $t.Name; Ok = $ok; Clippy = $clippyStatus }
    if (-not $ok) {
        Write-Host "失敗: $($t.Name)" -ForegroundColor Red
    }
}

Write-Host ""
Write-Host "=== 結果 ===" -ForegroundColor Cyan
$results | ForEach-Object {
    $mark = if ($_.Ok) { "OK  " } else { "FAIL" }
    $color = if ($_.Ok) { "Green" } else { "Red" }
    Write-Host "[$mark] $($_.Target)  (clippy 警告: $($_.Clippy))" -ForegroundColor $color
}

if ($results | Where-Object { -not $_.Ok }) {
    exit 1
}
exit 0
