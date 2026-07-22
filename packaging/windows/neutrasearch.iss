#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef BinDir
  #define BinDir "..\..\target\x86_64-pc-windows-msvc\release"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{AEE62A24-7B29-43EC-87DD-5433AE8CF4C6}
AppName=Neutrasearch
AppVersion={#AppVersion}
AppPublisher=NetroAki
AppPublisherURL=https://github.com/NetroAki/neutrasearch
AppSupportURL=https://github.com/NetroAki/neutrasearch/issues
AppUpdatesURL=https://github.com/NetroAki/neutrasearch/releases
DefaultDirName={autopf}\Neutrasearch
DefaultGroupName=Neutrasearch
DisableDirPage=yes
DisableProgramGroupPage=yes
UsePreviousAppDir=no
LicenseFile=..\..\LICENSE
OutputDir={#OutputDir}
OutputBaseFilename=neutrasearch-{#AppVersion}-windows-x64-setup
SetupIconFile=..\..\crates\neutra-gui\assets\neutrasearch.ico
UninstallDisplayIcon={app}\neutrasearch.exe
Compression=lzma2/ultra64
SolidCompression=yes
WizardStyle=modern
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
CloseApplications=yes
RestartApplications=no

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Additional shortcuts:"; Flags: unchecked

[Files]
Source: "{#BinDir}\neutrasearch.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\neutrasearch-helper.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\neutrasearch-query.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#BinDir}\neutrasearch-mcp.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\SECURITY.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "install-service.ps1"; DestDir: "{app}\service"; Flags: ignoreversion
Source: "remove-service.ps1"; DestDir: "{app}\service"; Flags: ignoreversion

[Icons]
Name: "{group}\Neutrasearch"; Filename: "{app}\neutrasearch.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Neutrasearch"; Filename: "{app}\neutrasearch.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\neutrasearch.exe"; Description: "Launch Neutrasearch"; Flags: nowait postinstall skipifsilent

[Code]
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Parameters: String;
begin
  { Stop and wait before [Files] replaces the service executable. }
  ResultCode := -1;
  Parameters := '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$s = Get-Service -Name ''NeutrasearchHelper'' -ErrorAction SilentlyContinue; if ($null -ne $s -and $s.Status -ne ''Stopped'') { Stop-Service -Name ''NeutrasearchHelper'' -Force; $s.WaitForStatus(''Stopped'', [TimeSpan]::FromSeconds(30)) }"';
  if (not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    Parameters, '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
    Result := Format('The existing Neutrasearch scanner service could not be stopped safely (exit code %d).', [ResultCode])
  else
  begin
    if not ForceDirectories(ExpandConstant('{app}')) then
    begin
      Result := 'The protected Neutrasearch installation directory could not be created.';
      exit;
    end;
    ResultCode := -1;
    if (not Exec(ExpandConstant('{sys}\icacls.exe'),
      '"' + ExpandConstant('{app}') + '" /reset /T /C /Q', '', SW_HIDE,
      ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
    begin
      Result := Format('The Neutrasearch installation ACL could not be reset (exit code %d).', [ResultCode]);
      exit;
    end;
    ResultCode := -1;
    if (not Exec(ExpandConstant('{sys}\icacls.exe'),
      '"' + ExpandConstant('{app}') + '" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)(F) *S-1-5-32-544:(OI)(CI)(F) *S-1-5-32-545:(OI)(CI)(RX) /T /C /Q',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
    begin
      Result := Format('The Neutrasearch installation ACL could not be hardened (exit code %d).', [ResultCode]);
      exit;
    end;
    Result := '';
  end;
end;

procedure CurStepChanged(CurStep: TSetupStep);
var
  ResultCode: Integer;
  Parameters: String;
begin
  if CurStep = ssPostInstall then
  begin
    ResultCode := -1;
    Parameters := ExpandConstant('-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{app}\service\install-service.ps1" -InstallDir "{app}"');
    if (not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      Parameters, '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
      RaiseException(Format('The Neutrasearch scanner service could not be installed (exit code %d).', [ResultCode]));
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
var
  ResultCode: Integer;
  Parameters: String;
begin
  if CurUninstallStep = usUninstall then
  begin
    ResultCode := -1;
    Parameters := ExpandConstant('-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "{app}\service\remove-service.ps1"');
    if (not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      Parameters, '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
      RaiseException(Format('The Neutrasearch scanner service could not be removed (exit code %d).', [ResultCode]));
  end;
end;
