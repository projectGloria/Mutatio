; Mutatio installer (Inno Setup 6). Build with installer\build.ps1, which compiles the release
; exe and the wizard images first. Version comes from the exe's VERSIONINFO, which build.rs fills
; from Cargo.toml — so Cargo.toml is the only place the version number lives.

#define AppName "Mutatio"
#define AppExe "mutatio.exe"
#define ExePath "..\target\release\mutatio.exe"
; Console-subsystem CLI, installed as mutatio.com (see src/cli.rs for why).
#define CliPath "..\target\release\mutatio-cli.exe"
#define AppVersion GetStringFileInfo(ExePath, "ProductVersion")

[Setup]
; Fixed AppId: upgrades and uninstall entries depend on it never changing.
AppId={{8F3C2B71-5E4A-4C1D-9B6E-2A7D0F1E9C54}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher=projectGloria
AppPublisherURL=https://github.com/projectGloria/Mutatio
AppSupportURL=https://github.com/projectGloria/Mutatio/issues
VersionInfoVersion={#AppVersion}
; Per-user install by default: the app only touches the current user's taskbar, so it needs no
; admin rights. Users can still opt into a machine-wide install from the dialog.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
DefaultDirName={autopf}\{#AppName}
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
LicenseFile=..\LICENSE
SetupIconFile=..\Mutatio.ico
UninstallDisplayIcon={app}\{#AppExe}
UninstallDisplayName={#AppName}
WizardStyle=modern
WizardImageFile=build\wizard.bmp
WizardSmallImageFile=build\wizard-small.bmp
OutputDir=..\dist
OutputBaseFilename=Mutatio-Setup-{#AppVersion}
Compression=lzma2/ultra
SolidCompression=yes
; Restart Manager closes a running Mutatio before files are replaced. The daemon handles the
; resulting WM_CLOSE by exiting cleanly and restoring the taskbar.
CloseApplications=yes
RestartApplications=no
; Broadcast WM_SETTINGCHANGE so newly opened terminals pick up the PATH change.
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "autostart"; Description: "Start {#AppName} when I sign in to Windows"; GroupDescription: "Startup:"
Name: "addtopath"; Description: "Add {#AppName} to PATH (run `mutatio --status` etc. from any terminal)"; GroupDescription: "Command line:"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#ExePath}"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#CliPath}"; DestDir: "{app}"; DestName: "mutatio.com"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\{#AppName}"; Filename: "{app}\{#AppExe}"
Name: "{autodesktop}\{#AppName}"; Filename: "{app}\{#AppExe}"; Tasks: desktopicon

[Registry]
; HKA = HKCU for per-user installs, HKLM for all-users installs.
Root: HKA; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "{#AppName}"; ValueData: """{app}\{#AppExe}"""; Flags: uninsdeletevalue; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "{cm:LaunchProgram,{#AppName}}"; Flags: nowait postinstall skipifsilent

[UninstallRun]
; Stop the daemon so it restores the stock taskbar before its files are removed.
Filename: "{app}\{#AppExe}"; Parameters: "--quit"; Flags: runhidden waituntilterminated; RunOnceId: "StopMutatio"

[UninstallDelete]
; ExplorerTAP.dll extracted at runtime. It may still be loaded by explorer.exe (locked) until the
; shell restarts; Inno skips locked files silently. The user's config in %APPDATA% is kept.
Type: filesandordirs; Name: "{localappdata}\Mutatio"

[Code]
const
  EnvKeyUser = 'Environment';
  EnvKeyMachine = 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment';

// PATH lives in HKCU for per-user installs and HKLM for all-users installs.
function EnvRoot(): Integer;
begin
  if IsAdminInstallMode() then Result := HKLM else Result := HKCU;
end;

function EnvKey(): String;
begin
  if IsAdminInstallMode() then Result := EnvKeyMachine else Result := EnvKeyUser;
end;

procedure AddToPath(Dir: String);
var
  Paths: String;
begin
  if not RegQueryStringValue(EnvRoot(), EnvKey(), 'Path', Paths) then Paths := '';
  // Delimited, case-insensitive check so "C:\A" doesn't match "C:\AB".
  if Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Paths) + ';') > 0 then exit;
  if (Paths <> '') and (Copy(Paths, Length(Paths), 1) <> ';') then Paths := Paths + ';';
  RegWriteExpandStringValue(EnvRoot(), EnvKey(), 'Path', Paths + Dir);
end;

procedure RemoveFromPath(Dir: String);
var
  Paths: String;
  P: Integer;
begin
  if not RegQueryStringValue(EnvRoot(), EnvKey(), 'Path', Paths) then exit;
  Paths := ';' + Paths + ';';
  P := Pos(';' + Uppercase(Dir) + ';', Uppercase(Paths));
  if P = 0 then exit;
  Delete(Paths, P, Length(Dir) + 1);
  Paths := Copy(Paths, 2, Length(Paths) - 2); // drop the sentinel semicolons
  RegWriteExpandStringValue(EnvRoot(), EnvKey(), 'Path', Paths);
end;

procedure CurStepChanged(CurStep: TSetupStep);
begin
  if (CurStep = ssPostInstall) and WizardIsTaskSelected('addtopath') then
    AddToPath(ExpandConstant('{app}'));
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  // Always attempt removal; harmless if the task wasn't selected at install time.
  if CurUninstallStep = usPostUninstall then
    RemoveFromPath(ExpandConstant('{app}'));
end;

// Upgrade path: ask an already-installed Mutatio to quit (and restore the taskbar) before
// Restart Manager has to step in.
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  ResultCode: Integer;
  ExistingExe: String;
begin
  ExistingExe := ExpandConstant('{app}\{#AppExe}');
  if FileExists(ExistingExe) then
    Exec(ExistingExe, '--quit', '', SW_HIDE, ewWaitUntilTerminated, ResultCode);
  Result := '';
end;
