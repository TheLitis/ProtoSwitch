#ifndef AppVersion
  #define AppVersion "0.1.0-beta.7"
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
AppPublisher=The_Litis
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
Name: "{autoprograms}\Удалить ProtoSwitch"; Filename: "{uninstallexe}"

[Run]
Filename: "{app}\protoswitch.exe"; Parameters: "init --non-interactive --no-autostart"; StatusMsg: "Настраивается ProtoSwitch..."; Flags: runhidden waituntilterminated runasoriginaluser
Filename: "{app}\protoswitch.exe"; Parameters: "autostart install"; StatusMsg: "Включается автозапуск ProtoSwitch..."; Flags: runhidden waituntilterminated runasoriginaluser; Check: ShouldEnableAutostart
Filename: "{app}\protoswitch.exe"; Description: "Запустить ProtoSwitch"; Flags: nowait postinstall runasoriginaluser skipifsilent

[UninstallRun]
Filename: "{app}\protoswitch.exe"; Parameters: "autostart remove"; Flags: runhidden waituntilterminated skipifdoesntexist; RunOnceId: "ProtoSwitchRemoveAutostart"

[UninstallDelete]
Type: files; Name: "{autodesktop}\ProtoSwitch.lnk"

[Code]
var
  DesktopShortcutCheckBox: TNewCheckBox;

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

procedure InitializeWizard;
begin
  DesktopShortcutCheckBox := TNewCheckBox.Create(WizardForm);
  DesktopShortcutCheckBox.Parent := WizardForm.FinishedLabel.Parent;
  DesktopShortcutCheckBox.Caption := 'Добавить ярлык ProtoSwitch на рабочий стол';
  DesktopShortcutCheckBox.Checked := True;
  DesktopShortcutCheckBox.Width := WizardForm.FinishedLabel.Width;
  DesktopShortcutCheckBox.Visible := False;
end;

procedure CurPageChanged(CurPageID: Integer);
begin
  if CurPageID = wpFinished then begin
    if WizardForm.RunList.Visible then
      DesktopShortcutCheckBox.Top := WizardForm.RunList.Top + WizardForm.RunList.Height + ScaleY(8)
    else
      DesktopShortcutCheckBox.Top := WizardForm.FinishedLabel.Top + WizardForm.FinishedLabel.Height + ScaleY(16);
    DesktopShortcutCheckBox.Left := WizardForm.FinishedLabel.Left;
    DesktopShortcutCheckBox.Visible := not WizardSilent;
  end else begin
    DesktopShortcutCheckBox.Visible := False;
  end;
end;

procedure CreateDesktopShortcut;
var
  Shell: Variant;
  Shortcut: Variant;
begin
  Shell := CreateOleObject('WScript.Shell');
  Shortcut := Shell.CreateShortcut(ExpandConstant('{autodesktop}\ProtoSwitch.lnk'));
  Shortcut.TargetPath := ExpandConstant('{app}\protoswitch.exe');
  Shortcut.WorkingDirectory := ExpandConstant('{app}');
  Shortcut.IconLocation := ExpandConstant('{app}\protoswitch.exe,0');
  Shortcut.Save;
end;

function NextButtonClick(CurPageID: Integer): Boolean;
begin
  Result := True;
  if (CurPageID = wpFinished) and DesktopShortcutCheckBox.Visible and DesktopShortcutCheckBox.Checked then
    CreateDesktopShortcut;
end;
