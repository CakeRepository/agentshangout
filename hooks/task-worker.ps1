# Does one task for the doorbell (OT1 to OT10). The doorbell starts this when the bot's owner writes
# "@Bot task: ..." in a room, with the task on stdin, TASK_ID and AGENTHANGOUT_TASK_REPO
# (owner/name) in a scrubbed environment.
#
# This is not the bot in the room. That bot has only the room's tools because room text comes from
# other people's bots. This run never reads a room: its only input is the owner's own words. It gets
# a fresh worktree of the repository, a short list of tools, and no token for the site.
#
# This is not a sandbox. `cargo test` runs code from the worktree with your rights, so anything the
# task can get into the repository, it can run as you. The rules below stop the obvious mistakes.
#
# The last line it prints is the pull request address or the word FAILED, and nothing else goes to
# stdout. The doorbell posts the address in the room and ignores the rest. Everything else goes to
# ~/.agenthangout/tasks/<id>.log.
$ErrorActionPreference = 'Stop'
[Console]::InputEncoding = [Text.UTF8Encoding]::new($false)
$OutputEncoding = [Text.UTF8Encoding]::new($false)

$task = [Console]::In.ReadToEnd()
$id = ([string]$env:TASK_ID) -replace '[^A-Za-z0-9_-]', '-'
$repoName = [string]$env:AGENTHANGOUT_TASK_REPO
$logs = Join-Path $HOME '.agenthangout\tasks'
New-Item -ItemType Directory -Force $logs | Out-Null
if (-not $id -or -not $task.Trim() -or $repoName -notmatch '^[A-Za-z0-9-]+/[A-Za-z0-9._-]+$') {
    'FAILED'
    exit 1
}
$log = Join-Path $logs "$id.log"
function Log([string]$line) { Add-Content -LiteralPath $log -Value "$(Get-Date -Format o) $line" }

$repo = 'T:\agentshangout'
$root = 'T:\ah-tasks'   # short, because long paths break the MSVC linker
$tree = Join-Path $root $id
$branch = "task/$id"
$pr = "^https://github\.com/$([regex]::Escape($repoName))/pull/[0-9]+$"

# Only ever in this repository, so a task can't point this at some other one.
$origin = (& git -C $repo config --get remote.origin.url 2>$null)
if ($LASTEXITCODE -ne 0 -or $origin -notmatch ('github\.com[:/]' + [regex]::Escape($repoName) + '(\.git)?/?$')) {
    Log "${repo} is not a checkout of ${repoName}"
    'FAILED'
    exit 1
}

# Never sweep prior task directories: a restarted doorbell may leave a worker running,
# and an unpublished branch may be the only copy of its work. Retain this task's tree too.
# The owner can inspect and remove completed worktrees explicitly. The free-space gate
# below stops new tasks when retained work consumes the available budget.

try {
    Log "task $id"
    # A build here can take many GB, and running a disk dry breaks every other build on it.
    $env:CARGO_INCREMENTAL = '0'
    if ((Get-PSDrive ($root.Substring(0, 1))).Free -lt 8GB) { throw 'less than 8 GB free' }
    & git -C $repo fetch origin main *>> $log
    if ($LASTEXITCODE -ne 0) { throw 'git fetch failed' }
    New-Item -ItemType Directory -Force $root | Out-Null
    & git -C $repo worktree add -b $branch $tree origin/main *>> $log
    if ($LASTEXITCODE -ne 0) { throw 'git worktree add failed' }

    # Files only inside this worktree (./ is where claude runs), not .git, which holds the hooks and
    # the link back to the main checkout. Git and cargo only in the forms a change needs. No
    # merging, no releases, no workflows, no API calls, no curl, nothing aimed at another repo,
    # and none of the places secrets live.
    $allow = @(
        'Read(./**)', 'Edit(./**)',
        'Bash(git status:*)', 'Bash(git diff:*)', 'Bash(git log:*)', 'Bash(git show:*)',
        'Bash(git add:*)', 'Bash(git commit:*)', 'Bash(git switch:*)', 'Bash(git branch:*)',
        'Bash(git fetch:*)', 'Bash(git push origin HEAD)', 'Bash(git push -u origin HEAD)',
        'Bash(cargo fmt:*)', 'Bash(cargo clippy:*)', 'Bash(cargo test:*)', 'Bash(cargo build:*)',
        'Bash(cargo check:*)',
        'Bash(gh issue view:*)', 'Bash(gh issue list:*)', 'Bash(gh pr create:*)',
        'Bash(gh pr view:*)', 'Bash(gh pr checks:*)'
    )
    $deny = @(
        'Edit(./.git)', 'Edit(./.git/**)', 'Read(./.git/config)',
        'Bash(git push origin main*)', 'Bash(git push*:main*)', 'Bash(git push --force*)',
        'Bash(git push -f*)', 'Bash(git diff*--no-index*)', 'Bash(git diff*--output*)',
        'Bash(gh pr create*--repo*)', 'Bash(gh pr create*-R *)',
        'Bash(gh pr create*--body-file*)', 'Bash(gh pr create*-F *)',
        'Bash(gh pr merge:*)', 'Bash(gh release:*)', 'Bash(gh workflow:*)', 'Bash(gh api:*)',
        'Bash(curl:*)',
        'Read(~/.agenthangout.env)', 'Read(~/.agenthangout/**)', 'Read(~/.claude.json)',
        'Read(~/.claude/**)', 'Read(~/.ssh/**)'
    )

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
    if ($last -notmatch $pr) { throw 'the last line was not a pull request address' }
    $last
    exit 0
} catch {
    Log "failed: $_"
    'FAILED'
    exit 1
}
