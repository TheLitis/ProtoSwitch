#ifndef AppVersion
  #define AppVersion "0.2.0-beta.3"
#endif
#ifndef RepoRoot
  #error RepoRoot must be defined.
#endif
#ifndef AppNumericVersion
  #error AppNumericVersion must be defined.
#endif
#ifndef ReleaseDir
  #error ReleaseDir must be defined.
#endif
#ifndef OutputDir
  #error OutputDir must be defined.
#endif

[Setup]
AppId={{D68BA0D1-DBD6-4608-99D8-A516A3725E1A}
AppName=ProtoSwitch
AppVersion={#AppVersion}
AppPublisher=TheLitis
AppPublisherURL=https://github.com/TheLitis/ProtoSwitch
AppSupportURL=https://github.com/TheLitis/ProtoSwitch
AppUpdatesURL=https://github.com/TheLitis/ProtoSwitch/releases
VersionInfoVersion={#AppNumericVersion}
DefaultDirName={code:GetInstallDir}
DefaultGroupName=ProtoSwitch
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64os
ArchitecturesInstallIn64BitMode=x64os
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
SetupLogging=yes
OutputDir={#OutputDir}
OutputBaseFilename=ProtoSwitch-Setup-x64
UninstallDisplayIcon={app}\protoswitch.exe
SetupIconFile={#RepoRoot}\assets\windows\protoswitch.ico

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "autostart"; Description: "Включить автозапуск watcher"

[Files]
Source: "{#ReleaseDir}\protoswitch.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\CHANGELOG.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#RepoRoot}\packaging\windows\QUICKSTART.txt"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\ProtoSwitch"; Filename: "{app}\protoswitch.exe"; IconFilename: "{app}\protoswitch.exe"
Name: "{autoprograms}\Починить ProtoSwitch"; Filename: "{app}\protoswitch.exe"; Parameters: "repair"; IconFilename: "{app}\protoswitch.exe"
Name: "{autoprograms}\Удалить ProtoSwitch"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\protoswitch.exe"; Parameters: "init --non-interactive --no-autostart"; StatusMsg: "Настраивается ProtoSwitch..."; Flags: runhidden waituntilterminated runasoriginaluser
Filename: "{app}\protoswitch.exe"; Parameters: "autostart install"; StatusMsg: "Включается автозапуск ProtoSwitch..."; Flags: runhidden waituntilterminated runasoriginaluser; Check: ShouldEnableAutostart
Filename: "powershell.exe"; Parameters: "-NoProfile -NonInteractive -WindowStyle Hidden -Command ""$ws = New-Object -ComObject WScript.Shell; $shortcut = $ws.CreateShortcut([Environment]::GetFolderPath('Desktop') + '\ProtoSwitch.lnk'); $shortcut.TargetPath = '{app}\protoswitch.exe'; $shortcut.WorkingDirectory = '{app}'; $shortcut.IconLocation = '{app}\protoswitch.exe,0'; $shortcut.Save()"""; Description: "Добавить ярлык ProtoSwitch на рабочий стол"; Flags: postinstall runhidden runasoriginaluser unchecked skipifsilent
Filename: "{app}\protoswitch.exe"; Description: "Запустить ProtoSwitch"; Flags: nowait postinstall runasoriginaluser skipifsilent

[UninstallRun]
Filename: "{app}\protoswitch.exe"; Parameters: "autostart remove"; Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "ProtoSwitchRemoveAutostart"

[UninstallDelete]
Type: files; Name: "{autodesktop}\ProtoSwitch.lnk"

[Code]
function GetInstallDir(Param: string): string;
begin
  if IsAdminInstallMode then
    Result := ExpandConstant('{autopf}\ProtoSwitch')
  else
    Result := ExpandConstant('{localappdata}\Programs\ProtoSwitch');
end;

function ShouldEnableAutostart: Boolean;
begin
  Result := WizardIsTaskSelected('autostart');
end;
