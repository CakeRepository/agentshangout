# Uploads the current source, builds it on the server and restarts the service.
# The first build takes several minutes; after that only changed code rebuilds.
param(
    [string]$ServerName = (((Get-Content "$PSScriptRoot\azure.local.env") -match '^PUBLIC_HOST=') -replace '^PUBLIC_HOST=', ''),
    [string]$User = 'azureuser',
    [string]$Key = "$HOME\.ssh\agenthangout_azure"
)
$ErrorActionPreference = 'Stop'
$PSNativeCommandUseErrorActionPreference = $true

$login = "$User@$ServerName"
$ssh = @('-i', $Key, '-o', 'StrictHostKeyChecking=accept-new')
$archive = Join-Path ([IO.Path]::GetTempPath()) 'agenthangout-src.tgz'

tar -czf $archive -C (Split-Path $PSScriptRoot) Cargo.toml Cargo.lock src web
scp @ssh $archive "${login}:agenthangout-src.tgz"
Remove-Item $archive

$build = @'
set -e
mkdir -p ~/agenthangout
cd ~/agenthangout
rm -rf src web
tar -xzf ~/agenthangout-src.tgz
~/.cargo/bin/cargo build --release --locked --bin agenthangout
sudo /usr/bin/install -m 755 target/release/agenthangout /usr/local/bin/agenthangout
sudo /usr/bin/systemctl restart agenthangout
'@
ssh @ssh $login ($build -replace "`r", '')
Write-Host "Released to https://$ServerName"
