# Does one task for the doorbell (L22 to L30). The doorbell starts this when the bot's owner writes
# "@Bot task: ..." in a room, with the task on stdin and TASK_ID in the environment.
#
# This is not the bot in the room. The bot in the room has only the room's tools because room text
# comes from other people's bots. This run never reads a room: its only input is the owner's own
# words. It gets a fresh worktree of agentshangout, a short list of tools, and no token for the site.
#
# Whatever happens, the last line it prints is the pull request address or the word FAILED, and
# nothing else goes to stdout. The doorbell posts that address in the room and ignores the rest.
# Everything else goes to ~/.agenthangout/tasks/<id>.log.
#
#   AGENTHANGOUT_TASK_REPO_DIR   the agentshangout checkout (default T:\agentshangout)
#   AGENTHANGOUT_TASK_ROOT       where worktrees go (default T:\ah-tasks). Keep it short: long paths
#                                break the MSVC linker.
$ErrorActionPreference = 'Stop'

$task = [Console]::In.ReadToEnd()
$id = ([string]$env:TASK_ID) -replace '[^A-Za-z0-9_-]', '-'
$logs = Join-Path $HOME '.agenthangout\tasks'
New-Item -ItemType Directory -Force $logs | Out-Null
if (-not $id -or -not $task.Trim()) {
    'FAILED'
    exit 1
}
$log = Join-Path $logs "$id.log"
function Log([string]$line) { Add-Content -LiteralPath $log -Value "$(Get-Date -Format o) $line" }

# The doorbell takes these out already. A worker that can't reach the site can't be talked into it.
Remove-Item -ErrorAction Ignore Env:AGENTHANGOUT_TOKEN, Env:AGENTHANGOUT_URL

$repo = if ($env:AGENTHANGOUT_TASK_REPO_DIR) { $env:AGENTHANGOUT_TASK_REPO_DIR } else { 'T:\agentshangout' }
$root = if ($env:AGENTHANGOUT_TASK_ROOT) { $env:AGENTHANGOUT_TASK_ROOT } else { 'T:\ah-tasks' }
$tree = Join-Path $root $id
$branch = "task/$id"
$pr = 'https://github.com/[A-Za-z0-9-]+/[A-Za-z0-9._-]+/pull/[0-9]+'

# Only ever in agentshangout, so a task can't point this at some other repository.
$origin = (& git -C $repo config --get remote.origin.url 2>$null)
if ($LASTEXITCODE -ne 0 -or $origin -notmatch 'github\.com[:/][A-Za-z0-9-]+/agentshangout(\.git)?/?$') {
    Log "${repo} is not an agentshangout checkout"
    'FAILED'
    exit 1
}

$mine = $false
function Clean-Up {
    # Keep the work if anything reached GitHub. Only a worktree that pushed nothing is thrown away.
    if (-not $mine) { return }
    & git -C $tree ls-remote --exit-code --heads origin $branch *>> $log
    if ($LASTEXITCODE -ne 2) {
        Log "keeping $tree (the branch is on GitHub, or this couldn't be checked)"
        return
    }
    & git -C $repo worktree remove --force $tree *>> $log
    & git -C $repo branch -D $branch *>> $log
    Log "removed ${tree}: nothing was pushed"
}

try {
    Log "task $id"
    & git -C $repo fetch origin main *>> $log
    if ($LASTEXITCODE -ne 0) { throw 'git fetch failed' }
    New-Item -ItemType Directory -Force $root | Out-Null
    & git -C $repo worktree add -b $branch $tree origin/main *>> $log
    if ($LASTEXITCODE -ne 0) { throw 'git worktree add failed' }
    $mine = $true

    # Tools: files in the worktree, git, cargo, and a little of gh. No merging, no releases, no
    # workflows, no API calls, no curl, no pushing to main, and none of the places secrets live.
    $allow = @(
        'Read', 'Glob', 'Grep', 'Edit', 'Write',
        'Bash(git:*)', 'Bash(cargo:*)',
        'Bash(gh issue view:*)', 'Bash(gh issue list:*)',
        'Bash(gh pr create:*)', 'Bash(gh pr view:*)', 'Bash(gh pr checks:*)'
    )
    $secrets = '~/.agenthangout.env', '~/.agenthangout/**', '~/.claude.json', '~/.claude/**',
        '~/.ssh/**', '~/.git-credentials', '~/.config/gh/**', '~/AppData/Roaming/GitHub CLI/**',
        '~/.aws/**', '~/.azure/**'
    $deny = @(
        'Bash(git push origin main*)', 'Bash(git push*:main*)', 'Bash(git push --force*)',
        'Bash(git push -f*)', 'Bash(git credential*)', 'Bash(git config*)',
        'Bash(gh pr merge:*)', 'Bash(gh release:*)', 'Bash(gh workflow:*)', 'Bash(gh api:*)',
        'Bash(curl:*)'
    )
    foreach ($path in $secrets) {
        $deny += "Read($path)", "Edit($path)", "Write($path)"
    }

    $noServers = Join-Path $logs 'no-mcp-servers.json'
    Set-Content -LiteralPath $noServers -Value '{"mcpServers":{}}'
    $orders = Get-Content -Raw -LiteralPath (Join-Path $PSScriptRoot 'task-worker-orders.md')

    $claude = @(
        '--print',
        '--model', 'claude-sonnet-5-5',
        '--max-budget-usd', '5',
        '--tools', 'Read,Glob,Grep,Edit,Write,Bash',
        '--strict-mcp-config',
        '--mcp-config', $noServers,
        '--setting-sources', '',
        '--permission-mode', 'dontAsk',
        '--allowedTools', $allow,
        '--disallowedTools', $deny,
        '--append-system-prompt', $orders,
        '--no-session-persistence'
    )
    Push-Location $tree
    try {
        $output = $task | & claude @claude 2>> $log
        $code = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    Log ($output -join "`n")
    if ($code -ne 0) { throw "claude exited with $code" }

    $last = ($output | ForEach-Object { "$_".Trim() } | Where-Object { $_ } | Select-Object -Last 1)
    if ($last -notmatch "^$pr$") { throw 'the last line was not a pull request address' }
    Clean-Up
    $last
    exit 0
} catch {
    Log "failed: $_"
    Clean-Up
    'FAILED'
    exit 1
}
