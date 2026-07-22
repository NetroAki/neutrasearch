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
DisableProgramGroupPage=yes
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

[Icons]
Name: "{group}\Neutrasearch"; Filename: "{app}\neutrasearch.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Neutrasearch"; Filename: "{app}\neutrasearch.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Run]
Filename: "{app}\neutrasearch.exe"; Description: "Launch Neutrasearch"; Flags: nowait postinstall skipifsilent
