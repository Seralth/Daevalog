; SPDX-License-Identifier: AGPL-3.0-only
;
; Inno Setup script for Daevalog DPS Meter 2.0 on Windows.
;
; It installs a folder that the build has already filled (see
; .github/workflows/windows.yml): daevalog-dps-meter.exe, the Qt runtime that
; windeployqt copied beside it, the Visual C++ runtime, the game data in data\
; and the icon. Build it with:
;
;   iscc /DSourceDir=<that folder> /DAppVersion=2.0.0 /DOutputDir=<out> daevalog-dps-meter.iss
;
; Npcap is not included: its free licence does not allow it. Setup only
; checks for it and tells the player where to get it.

#ifndef SourceDir
  #define SourceDir "..\..\stage"
#endif
#ifndef AppVersion
  #define AppVersion "2.0.0"
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#define AppName "Daevalog DPS Meter"
#define AppExe "daevalog-dps-meter.exe"

[Setup]
AppId={{0A70002E-9E5B-43BF-B9C6-9DA08D03996D}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=Seralth
AppPublisherURL=https://github.com/Seralth/Daevalog
AppSupportURL=https://github.com/Seralth/Daevalog/issues
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
LicenseFile=..\..\LICENSE
SetupIconFile=..\..\src-tauri\icons\icon.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
OutputDir={#OutputDir}
OutputBaseFilename=daevalog-dps-meter-{#AppVersion}-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
PrivilegesRequired=admin
CloseApplications=yes

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\{#AppName}"; Filename: "{app}\{#AppExe}"; IconFilename: "{app}\daevalog.ico"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\{#AppExe}"; Description: "Start {#AppName}"; Flags: nowait postinstall skipifsilent

[Code]
const
  NpcapUrl = 'https://npcap.com';

// Npcap puts wpcap.dll in System32\Npcap.
function NpcapInstalled: Boolean;
begin
  Result := FileExists(ExpandConstant('{sys}\Npcap\wpcap.dll'));
end;

function InitializeSetup: Boolean;
var
  ErrorCode: Integer;
begin
  Result := True;
  if NpcapInstalled or WizardSilent then
    Exit;
  if MsgBox('Npcap is not installed.' + #13#10#13#10 +
      'The meter reads the game''s network traffic through Npcap. Its licence ' +
      'does not allow this installer to include it, so install it yourself ' +
      'from ' + NpcapUrl + ' (the free edition works). You can do that before ' +
      'or after this setup.' + #13#10#13#10 +
      'Open ' + NpcapUrl + ' now?', mbConfirmation, MB_YESNO) = IDYES then
    ShellExec('open', NpcapUrl, '', '', SW_SHOWNORMAL, ewNoWait, ErrorCode);
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if (CurPageID = wpFinished) and not NpcapInstalled then
    WizardForm.FinishedLabel.Caption := WizardForm.FinishedLabel.Caption + #13#10#13#10 +
      'Npcap is still missing. Install it from ' + NpcapUrl +
      ', then start the meter. Without it the meter cannot see the game''s traffic.';
end;
