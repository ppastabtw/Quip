param(
    [ValidateRange(1, 65535)]
    [int]$Port = 8765
)

$quipRepo = (Resolve-Path (Join-Path $PSScriptRoot '..\..\..')).Path
$quipDrive = $quipRepo.Substring(0, 1).ToLowerInvariant()
$quipWslRepo = "/mnt/$quipDrive$($quipRepo.Substring(2).Replace('\', '/'))"
$quipPython = '/home/arioo/.local/share/quip-workstream1/.venv/bin/python'
$quipFlashRoot = "$quipWslRepo/training/flash"

wsl.exe -d Ubuntu -- test -x $quipPython
if ($LASTEXITCODE -ne 0) {
    Write-Error "The Quip Flash environment is missing at $quipPython"
    exit 1
}

Write-Host "Opening Quip model playground at http://127.0.0.1:$Port"
Write-Host 'Press Ctrl+C to stop.'
wsl.exe -d Ubuntu --cd $quipFlashRoot -- $quipPython prototype/server.py --port $Port
exit $LASTEXITCODE
