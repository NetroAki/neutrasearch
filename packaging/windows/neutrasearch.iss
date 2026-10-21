#ifndef AppVersion
  #error Pass the release version with /DAppVersion=x.y.z
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
Source: "..\..\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\..\THIRD-PARTY-NOTICES.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "install-service.ps1"; DestDir: "{app}\service"; Flags: ignoreversion
Source: "remove-service.ps1"; DestDir: "{app}\service"; Flags: ignoreversion

[Icons]
Name: "{group}\Neutrasearch"; Filename: "{app}\neutrasearch.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Neutrasearch"; Filename: "{app}\neutrasearch.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[UninstallDelete]
Type: filesandordirs; Name: "{app}\logs"

[Run]
Filename: "{app}\neutrasearch.exe"; Description: "Launch Neutrasearch"; Flags: nowait postinstall skipifsilent

[Code]
var
  ApprovedAccountPage: TInputQueryWizardPage;

// The app directory constant is unavailable until the directory is chosen, so
// the pages that run earlier look for a previous install in the default place.
function HelperAlreadyInstalled: Boolean;
begin
  Result := FileExists(ExpandConstant('{autopf}\Neutrasearch\neutrasearch-helper.exe'));
end;

procedure InitializeWizard;
begin
  ApprovedAccountPage := CreateInputQueryPage(wpWelcome,
    'Approve a Windows account', 'Who will use Neutrasearch?',
    'This account may search file names, sizes and dates across all drives. If you supplied another administrator''s password to install, enter the account that will use the application. Upgrades keep the previous approval unless you choose an account here.');
  ApprovedAccountPage.Add('Windows account (COMPUTER\name or DOMAIN\name; blank keeps the existing approval):', False);
  if not HelperAlreadyInstalled then
    ApprovedAccountPage.Values[0] := GetUserNameString;
end;

function ValidateApprovedAccount: String;
var
  Account: String;
  ResultCode: Integer;
begin
  Result := '';
  if WizardSilent then exit;
  Account := Trim(ApprovedAccountPage.Values[0]);
  if Account = '' then
  begin
    if not HelperAlreadyInstalled then
      Result := 'Choose the Windows account that will use Neutrasearch.';
    exit;
  end;
  if Pos('"', Account) <> 0 then
  begin
    Result := 'The Windows account name cannot contain a quotation mark.';
    exit;
  end;
  StringChangeEx(Account, '''', '''''', True);
  ResultCode := -1;
  if (not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
    '-NoLogo -NoProfile -NonInteractive -Command "try { $a = [Security.Principal.NTAccount]::new(''' + Account + '''); $s = $a.Translate([Security.Principal.SecurityIdentifier]); if (-not $s.IsAccountSid()) { exit 1 }; exit 0 } catch { exit 1 }"',
    '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
    Result := 'The Windows account could not be found. Use COMPUTER\name or DOMAIN\name.';
end;

function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  Parameters: String;
begin
  Result := ValidateApprovedAccount;
  if Result <> '' then exit;
  { Stop and wait before [Files] replaces the service executable. }
  ResultCode := -1;
  Parameters := '-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$ErrorActionPreference = ''Stop''; $s = Get-Service -Name ''NeutrasearchHelper'' -ErrorAction SilentlyContinue; if ($null -ne $s -and $s.Status -ne ''Stopped'') { sc.exe stop NeutrasearchHelper | Out-Null; if ($LASTEXITCODE -ne 0 -and $LASTEXITCODE -ne 1062) { exit $LASTEXITCODE }; $s.WaitForStatus(''Stopped'', [TimeSpan]::FromSeconds(30)) }; exit 0"';
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
      '"' + ExpandConstant('{app}') + '" /inheritance:r /grant:r *S-1-5-18:(OI)(CI)(F) *S-1-5-32-544:(OI)(CI)(F) *S-1-5-32-545:(OI)(CI)(RX) /C /Q',
      '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
    begin
      Result := Format('The Neutrasearch installation ACL could not be hardened (exit code %d).', [ResultCode]);
      exit;
    end;
    { The recursive reset above leaves child files inheriting effective ACEs. }
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
    if (not WizardSilent) and (Trim(ApprovedAccountPage.Values[0]) <> '') then
    begin
      if Pos('"', ApprovedAccountPage.Values[0]) <> 0 then
        RaiseException('The Windows account name cannot contain a quotation mark.');
      Parameters := Parameters + ' -AuthorizedUserAccount "' + Trim(ApprovedAccountPage.Values[0]) + '"';
    end;
    if (not Exec(ExpandConstant('{sys}\WindowsPowerShell\v1.0\powershell.exe'),
      Parameters, '', SW_HIDE, ewWaitUntilTerminated, ResultCode)) or (ResultCode <> 0) then
      RaiseException(Format('The Neutrasearch scanner service could not be installed (exit code %d). See the logs folder in the installation directory (install-service.log) for details.', [ResultCode]));
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
