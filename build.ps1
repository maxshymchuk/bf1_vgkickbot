[CmdletBinding()]
param([switch]$Release, [switch]$Check)

$ErrorActionPreference = 'Stop'
if ($Release -and $Check) { throw 'Use either -Release or -Check.' }
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT -or
    -not [Environment]::Is64BitProcess) { throw 'Windows x64 and 64-bit PowerShell are required.' }

function Invoke-Cargo {
    param([string[]]$CargoArguments)
    & cargo @CargoArguments
    if ($LASTEXITCODE -ne 0) { throw ('cargo ' + ($CargoArguments -join ' ') + ' failed: ' + $LASTEXITCODE) }
}
function Write-Found {
    param([string]$Name, [string]$Path)
    Write-Host "[found] $Name at $Path; using it without downloading."
}
function Find-CargoVcpkg {
    param([string]$Version, [string]$LocalExe)
    $candidates = @((Get-Command cargo-vcpkg.exe -CommandType Application -All -ErrorAction SilentlyContinue).Source) + @($LocalExe)
    foreach ($candidate in ($candidates | Where-Object { $_ } | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
        try { $output = & $candidate --version 2>$null }
        catch { Write-Host "[skip] Cannot run cargo-vcpkg at $candidate."; continue }
        if ($LASTEXITCODE -eq 0 -and ($output -join "`n").Trim() -eq "cargo-vcpkg $Version") { return $candidate }
        Write-Host "[skip] cargo-vcpkg at $candidate does not match required version $Version."
    }
}
function Test-NativePackages {
    param([string]$Root, [string]$Triplet, $Versions, [string[]]$Dependencies)
    $statusPath = Join-Path $Root 'installed/vcpkg/status'
    if (-not (Test-Path -LiteralPath (Join-Path $Root 'vcpkg.exe') -PathType Leaf) -or
        -not (Test-Path -LiteralPath $statusPath -PathType Leaf)) { return $false }
    $records = @{}
    foreach ($paragraph in ([IO.File]::ReadAllText($statusPath) -split '\r?\n\r?\n')) {
        $fields = @{}
        foreach ($line in ($paragraph -split '\r?\n')) {
            if ($line -match '^([^:]+): (.*)$') { $fields[$Matches[1]] = $Matches[2] }
        }
        if ($fields['Architecture'] -eq $Triplet -and $fields['Status'] -eq 'install ok installed') {
            $feature = $fields['Feature']
            if (-not $feature) { $feature = 'core' }
            $records[$fields['Package'] + ':' + $feature] = $fields
        }
    }
    foreach ($version in $Versions.PSObject.Properties) {
        $core = $records[$version.Name + ':core']
        if (-not $core -or $core['Version'] -ne $version.Value) { return $false }
        $pattern = $version.Name + '_*_' + $Triplet + '.list'
        $manifests = @(Get-ChildItem -LiteralPath (Join-Path $Root 'installed/vcpkg/info') -Filter $pattern -ErrorAction SilentlyContinue)
        if (-not $manifests) { return $false }
        foreach ($manifest in $manifests) {
            foreach ($file in (Get-Content -LiteralPath $manifest.FullName)) {
                if ($file -match ('^' + [regex]::Escape($Triplet) + '/(?:lib/[^/]+\.lib|include/.+\.h(?:pp)?)$') -and
                    -not (Test-Path -LiteralPath (Join-Path (Join-Path $Root 'installed') $file) -PathType Leaf)) { return $false }
            }
        }
    }
    foreach ($dependency in $Dependencies) {
        if ($dependency -match '^([^\[]+)\[([^\]]+)\]$') {
            $port = $Matches[1]
            foreach ($feature in ($Matches[2] -split ',')) {
                if (-not $records[$port + ':' + $feature]) { return $false }
            }
        }
    }
    return $true
}

# Probe the real DLL version and loader dependencies in PowerShell 5.1 and 7.
if (-not ('BuildLibClangProbe' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class BuildLibClangProbe {
    [StructLayout(LayoutKind.Sequential)] struct CXString { public IntPtr data; public uint flags; }
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate CXString GetVersion();
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate IntPtr GetString(CXString value);
    [UnmanagedFunctionPointer(CallingConvention.Cdecl)] delegate void DisposeString(CXString value);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr LoadLibraryEx(string path, IntPtr file, uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Ansi, ExactSpelling = true)]
    static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32.dll")] static extern bool FreeLibrary(IntPtr module);
    public static string Version(string path) {
        IntPtr module = LoadLibraryEx(path, IntPtr.Zero, 0x1100);
        if (module == IntPtr.Zero) return null;
        try {
            IntPtr v = GetProcAddress(module, "clang_getClangVersion");
            IntPtr s = GetProcAddress(module, "clang_getCString");
            IntPtr d = GetProcAddress(module, "clang_disposeString");
            if (v == IntPtr.Zero || s == IntPtr.Zero || d == IntPtr.Zero) return null;
            var getVersion = (GetVersion)Marshal.GetDelegateForFunctionPointer(v, typeof(GetVersion));
            var getString = (GetString)Marshal.GetDelegateForFunctionPointer(s, typeof(GetString));
            var dispose = (DisposeString)Marshal.GetDelegateForFunctionPointer(d, typeof(DisposeString));
            CXString value = getVersion();
            try { return Marshal.PtrToStringAnsi(getString(value)); } finally { dispose(value); }
        } finally { FreeLibrary(module); }
    }
}
'@
}
function Find-Clang {
    param([string[]]$Candidates, [string]$Version)
    foreach ($candidate in ($Candidates | Where-Object { $_ } | Select-Object -Unique)) {
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
        try { $output = & $candidate --version 2>$null }
        catch { Write-Host "[skip] Cannot run Clang at $candidate."; continue }
        if ($LASTEXITCODE -eq 0 -and ($output -join "`n") -match ('clang version ' + [regex]::Escape($Version) + '(?:\s|$)')) {
            $resourceDir = (& $candidate -print-resource-dir 2>$null | Select-Object -Last 1)
            if ($LASTEXITCODE -eq 0 -and $resourceDir -and (Test-Path -LiteralPath (Join-Path $resourceDir 'include/stddef.h') -PathType Leaf)) { return $candidate }
        }
        Write-Host "[skip] Clang at $candidate is incomplete or does not match LLVM $Version."
    }
}
function Find-LibClang {
    param([string[]]$Candidates, [string]$Version)
    foreach ($candidate in ($Candidates | Where-Object { $_ } | Select-Object -Unique)) {
        if (Test-Path -LiteralPath $candidate -PathType Container) { $candidate = Join-Path $candidate 'libclang.dll' }
        if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
        $output = [BuildLibClangProbe]::Version([IO.Path]::GetFullPath($candidate))
        if ($output -and $output -match ('clang version ' + [regex]::Escape($Version) + '(?:\s|$)')) { return $candidate }
        Write-Host "[skip] libclang at $candidate cannot load or does not match LLVM $Version."
    }
}

$savedEnvironment = @{}
foreach ($name in @('PATH', 'CLANG_PATH', 'LIBCLANG_PATH', 'VCPKG_ROOT', 'VCPKGRS_TRIPLET')) {
    $savedEnvironment[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
}
Push-Location -LiteralPath $PSScriptRoot
try {
    foreach ($name in @('cargo', 'rustc', 'git')) {
        $command = Get-Command $name -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if (-not $command) { throw "Install $name first and open a new terminal. See README.md." }
        Write-Found $name $command.Source
    }
    $rustVersion = & rustc --version
    if ($LASTEXITCODE -ne 0 -or $rustVersion -notmatch '^rustc (\d+\.\d+\.\d+)' -or [version]$Matches[1] -lt [version]'1.95.0') { throw 'Rust 1.95 or newer is required; update the MSVC toolchain with rustup.' }
    $rustDetails = & rustc -vV
    if ($LASTEXITCODE -ne 0 -or -not ($rustDetails -match '^host: x86_64-pc-windows-msvc$')) { throw 'Use the x86_64-pc-windows-msvc Rust toolchain.' }
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio/Installer/vswhere.exe'
    $msvc = @()
    if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
        $msvc = @(& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -find 'VC\Tools\MSVC\**\bin\Hostx64\x64\cl.exe')
    }
    if (-not $msvc) { $msvc = @((Get-Command cl.exe -CommandType Application -ErrorAction SilentlyContinue).Source) }
    if (-not $msvc) { throw 'Install MSVC C++ x64/x86 Build Tools and a Windows SDK first. See README.md.' }
    Write-Found 'MSVC C++ tools' $msvc[0]
    $sdkInclude = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits/10/Include'
    if ($env:WindowsSdkDir) { $sdkInclude = Join-Path $env:WindowsSdkDir 'Include' }
    $sdk = Get-ChildItem -LiteralPath $sdkInclude -Directory -ErrorAction SilentlyContinue |
        Where-Object { Test-Path -LiteralPath (Join-Path $_.FullName 'um/Windows.h') -PathType Leaf } |
        Sort-Object Name -Descending | Select-Object -First 1
    if (-not $sdk) { throw 'Install a Windows 10/11 SDK through Visual Studio Installer. See README.md.' }
    Write-Found 'Windows SDK headers' $sdk.FullName

    $metadataJson = & cargo metadata --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) { throw 'Cannot read Cargo.toml metadata.' }
    $metadata = ($metadataJson -join "`n") | ConvertFrom-Json
    $package = $metadata.packages | Where-Object { $_.name -eq 'vgkickbot' }
    $buildTools = $package.metadata.'build-tools'
    $triplet = $package.metadata.vcpkg.target.'x86_64-pc-windows-msvc'.triplet
    $localVcpkg = Join-Path $PSScriptRoot 'target/vcpkg'
    $rootCandidates = @($savedEnvironment['VCPKG_ROOT'])
    $rootCandidates += @((Get-Command vcpkg.exe -CommandType Application -All -ErrorAction SilentlyContinue).Source | Where-Object { $_ } | ForEach-Object { Split-Path -Parent $_ })
    $rootCandidates += @((Join-Path $env:USERPROFILE 'vcpkg'), 'C:\vcpkg', $localVcpkg)
    $vcpkgRoot = $null
    foreach ($root in ($rootCandidates | Where-Object { $_ } | Select-Object -Unique)) {
        if (Test-NativePackages $root $triplet $buildTools.'native-versions' $package.metadata.vcpkg.dependencies) {
            $vcpkgRoot = [IO.Path]::GetFullPath($root)
            Write-Found "vcpkg packages ($triplet)" $vcpkgRoot
            foreach ($version in $buildTools.'native-versions'.PSObject.Properties) { Write-Host "[found] $($version.Name) $($version.Value); using installed package." }
            break
        }
        if (Test-Path -LiteralPath (Join-Path $root 'vcpkg.exe')) { Write-Host "[skip] vcpkg at $root has an incomplete or incompatible native package set." }
    }
    $env:VCPKGRS_TRIPLET = $triplet
    if (-not $vcpkgRoot) {
        $toolRoot = Join-Path $PSScriptRoot 'target/build-tools/cargo'
        $toolExe = Find-CargoVcpkg $buildTools.'cargo-vcpkg' (Join-Path $toolRoot 'bin/cargo-vcpkg.exe')
        if (-not $toolExe) {
            Write-Host "[prepare] cargo-vcpkg $($buildTools.'cargo-vcpkg') is missing; installing locally."
            Invoke-Cargo @('install', 'cargo-vcpkg', '--version', $buildTools.'cargo-vcpkg', '--locked', '--root', $toolRoot, '--force')
            $toolExe = Join-Path $toolRoot 'bin/cargo-vcpkg.exe'
        } else { Write-Found "cargo-vcpkg $($buildTools.'cargo-vcpkg')" $toolExe }
        $env:PATH = (Split-Path -Parent $toolExe) + [IO.Path]::PathSeparator + $savedEnvironment['PATH']
        $vcpkgRoot = $localVcpkg
        $env:VCPKG_ROOT = $vcpkgRoot
        Write-Host '[prepare] Native packages are missing; cargo-vcpkg will prepare the local installation and reuse its existing packages/tools.'
        Invoke-Cargo @('vcpkg', 'build')
        if (-not (Test-NativePackages $vcpkgRoot $triplet $buildTools.'native-versions' $package.metadata.vcpkg.dependencies)) { throw 'vcpkg did not prepare the required package versions and features.' }
    }
    $env:VCPKG_ROOT = $vcpkgRoot

    $llvmVersion = $buildTools.'llvm-version'
    $llvmMajor = ($llvmVersion -split '\.')[0]
    $llvmRoot = Join-Path $PSScriptRoot 'target/build-tools/llvm'
    $llvmBin = Join-Path $llvmRoot 'bin'
    $systemBins = @((Join-Path $env:ProgramFiles 'LLVM/bin'))
    if (Test-Path -LiteralPath $vswhere -PathType Leaf) {
        $vsPath = (& $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
        if ($vsPath) { $systemBins += @((Join-Path $vsPath 'VC/Tools/Llvm/x64/bin'), (Join-Path $vsPath 'VC/Tools/Llvm/bin')) }
    }
    $clangCandidates = @($savedEnvironment['CLANG_PATH']) + @((Get-Command clang.exe -CommandType Application -All -ErrorAction SilentlyContinue).Source)
    $clangCandidates += @($systemBins | ForEach-Object { Join-Path $_ 'clang.exe' }) + @(Join-Path $llvmBin 'clang.exe')
    $clangExe = Find-Clang $clangCandidates $llvmVersion
    $dllCandidates = @($savedEnvironment['LIBCLANG_PATH'])
    if ($clangExe) { $dllCandidates += Join-Path (Split-Path -Parent $clangExe) 'libclang.dll' }
    $dllCandidates += @($systemBins | ForEach-Object { Join-Path $_ 'libclang.dll' }) + @(Join-Path $llvmBin 'libclang.dll')
    $libclang = Find-LibClang $dllCandidates $llvmVersion
    if ($clangExe) { Write-Found "Clang $llvmVersion and builtin headers" $clangExe }
    if ($libclang) { Write-Found "libclang $llvmVersion" $libclang }

    if (-not $clangExe -or -not $libclang) {
        $downloadRoot = Join-Path $PSScriptRoot 'target/build-tools/downloads'
        New-Item -ItemType Directory -Path $downloadRoot -Force | Out-Null
        $archiveBase = "clang+llvm-$llvmVersion-x86_64-pc-windows-msvc"
        $archiveName = "$archiveBase.tar.zst"
        $archivePath = Join-Path $downloadRoot $archiveName
        $archiveReady = (Test-Path -LiteralPath $archivePath -PathType Leaf) -and (Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -eq $buildTools.'llvm-archive-sha256'
        if ($archiveReady) { Write-Found 'verified LLVM archive cache' $archivePath }
        else {
            Write-Host "[download] Missing LLVM components; downloading official LLVM $llvmVersion archive."
            $archiveUrl = "https://github.com/llvm/llvm-project/releases/download/llvmorg-$llvmVersion/" + [Uri]::EscapeDataString($archiveName)
            & curl.exe --fail --location --retry 3 --output $archivePath $archiveUrl
            if ($LASTEXITCODE -ne 0) { throw 'LLVM archive download failed.' }
            if ((Get-FileHash -LiteralPath $archivePath -Algorithm SHA256).Hash -ne $buildTools.'llvm-archive-sha256') { throw 'LLVM archive SHA-256 mismatch.' }
        }
        $sevenZipCandidates = @((Get-Command 7z.exe -CommandType Application -All -ErrorAction SilentlyContinue).Source)
        $sevenZipCandidates += @((Join-Path $env:ProgramFiles '7-Zip/7z.exe'), (Join-Path ${env:ProgramFiles(x86)} '7-Zip/7z.exe'))
        $sevenZip = $null
        foreach ($candidate in ($sevenZipCandidates | Where-Object { $_ } | Select-Object -Unique)) {
            if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) { continue }
            $output = & $candidate i 2>$null
            if ($LASTEXITCODE -eq 0 -and ($output -join "`n") -match '7-Zip (\d+\.\d+)' -and
                [version]$Matches[1] -ge [version]$buildTools.'7zip-min-version') { $sevenZip = $candidate; break }
        }
        if ($sevenZip) { Write-Found '7-Zip archive extractor' $sevenZip }
        else {
            Write-Host '[prepare] A suitable installed 7-Zip was not found; vcpkg will prepare or reuse its local tool.'
            $sevenZipOutput = & (Join-Path $vcpkgRoot 'vcpkg.exe') fetch 7zip
            if ($LASTEXITCODE -ne 0) { throw 'Cannot prepare the vcpkg 7-Zip tool.' }
            $sevenZip = ($sevenZipOutput | Select-Object -Last 1).Trim()
            Write-Host "[ready] 7-Zip archive extractor at $sevenZip."
        }
        & $sevenZip x $archivePath "-o$downloadRoot" -y
        if ($LASTEXITCODE -ne 0) { throw 'Cannot decompress the LLVM archive.' }
        $tarPath = Join-Path $downloadRoot "$archiveBase.tar"
        # Do not use -spf: archive entries must stay inside the extract directory.
        $extractRoot = Join-Path $downloadRoot 'llvm-unpacked'
        $members = @()
        if (-not $clangExe) { $members += @("$archiveBase/bin/clang.exe", "$archiveBase/lib/clang/$llvmMajor/include/*") }
        if (-not $libclang) { $members += "$archiveBase/bin/libclang.dll" }
        $extractArguments = @('x', $tarPath) + $members + @("-o$extractRoot", '-y')
        & $sevenZip @extractArguments
        if ($LASTEXITCODE -ne 0) { throw 'Cannot extract the missing LLVM components.' }
        New-Item -ItemType Directory -Path $llvmBin -Force | Out-Null
        if (-not $clangExe) {
            $clangExe = Join-Path $llvmBin 'clang.exe'
            Move-Item -LiteralPath (Join-Path $extractRoot "$archiveBase/bin/clang.exe") -Destination $clangExe -Force
            $headerParent = Join-Path $llvmRoot "lib/clang/$llvmMajor"
            New-Item -ItemType Directory -Path $headerParent -Force | Out-Null
            Copy-Item -LiteralPath (Join-Path $extractRoot "$archiveBase/lib/clang/$llvmMajor/include") -Destination $headerParent -Recurse -Force
            if ((Get-FileHash -LiteralPath $clangExe -Algorithm SHA256).Hash -ne $buildTools.'clang-sha256') { throw 'Clang executable SHA-256 mismatch.' }
            Write-Host "[ready] Prepared Clang and headers at $llvmRoot."
        }
        if (-not $libclang) {
            $libclang = Join-Path $llvmBin 'libclang.dll'
            Move-Item -LiteralPath (Join-Path $extractRoot "$archiveBase/bin/libclang.dll") -Destination $libclang -Force
            if ((Get-FileHash -LiteralPath $libclang -Algorithm SHA256).Hash -ne $buildTools.'libclang-sha256') { throw 'libclang DLL SHA-256 mismatch.' }
            Write-Host "[ready] Prepared libclang at $libclang."
        }
        $licensePath = Join-Path $llvmRoot 'LICENSE.txt'
        if (-not (Test-Path -LiteralPath $licensePath)) {
            Invoke-WebRequest -Uri "https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-$llvmVersion/llvm/LICENSE.TXT" -OutFile $licensePath -UseBasicParsing
        }
        Remove-Item -LiteralPath $tarPath
        if (-not (Find-Clang @($clangExe) $llvmVersion) -or -not (Find-LibClang @($libclang) $llvmVersion)) { throw 'Prepared LLVM tools failed validation.' }
    }
    $env:CLANG_PATH = $clangExe
    $env:LIBCLANG_PATH = Split-Path -Parent $libclang
    $buildArguments = @('build')
    if ($Check) { $buildArguments = @('check') }
    if ($Release) { $buildArguments += '--release' }
    Invoke-Cargo $buildArguments
}
finally {
    foreach ($name in $savedEnvironment.Keys) { [Environment]::SetEnvironmentVariable($name, $savedEnvironment[$name], 'Process') }
    Pop-Location
}
