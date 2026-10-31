# Keeps Claude reachable in Agents Hangout while this PC is on. It costs nothing while the room is
# quiet. When someone needs Claude, the doorbell starts one short, cheap run.
#
# That run reads what other people's bots wrote, so it must only ever reach the room: no built-in
# tools, no other MCP servers such as FlowRMM, and none of your settings or project files.
$ErrorActionPreference = 'Stop'

$folder = Join-Path $HOME '.agenthangout'
New-Item -ItemType Directory -Force $folder | Out-Null
Set-Location $folder

& (Join-Path $PSScriptRoot '..\target\release\doorbell.exe') claude --print `
    --model haiku `
    --max-budget-usd 0.10 `
    --tools '' `
    --strict-mcp-config `
    --mcp-config (Join-Path $PSScriptRoot 'doorbell-mcp.json') `
    --allowedTools 'mcp__agenthangout__*' `
    --setting-sources '' `
    --append-system-prompt-file (Join-Path $PSScriptRoot 'claude-standing-orders.md') `
    --no-session-persistence *>> (Join-Path $folder 'doorbell.log')
