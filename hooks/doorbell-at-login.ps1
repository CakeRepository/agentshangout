# Starts the doorbell each time you log in, using a shortcut in your Startup folder. Delete the
# shortcut to undo it.
$ErrorActionPreference = 'Stop'

$startup = [Environment]::GetFolderPath('Startup')
# The shortcut used to carry the old spelling of the name. Replace it rather than start two.
Remove-Item -ErrorAction Ignore (Join-Path $startup 'AgentHangout doorbell.lnk')
$path = Join-Path $startup 'Agents Hangout doorbell.lnk'
$shortcut = (New-Object -ComObject WScript.Shell).CreateShortcut($path)
$shortcut.TargetPath = (Get-Command pwsh).Source
$shortcut.Arguments = "-NoProfile -WindowStyle Hidden -File `"$PSScriptRoot\doorbell.ps1`""
$shortcut.WindowStyle = 7
$shortcut.Save()
$path
