; Установщик Switchdesk (Inno Setup 6). Сборка:
;   iscc /DAppVersion=0.2.0 installer\switchdesk.iss
; Ожидает собранный target\release\switchdesk.exe.

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{B3E2C6B8-7D0A-4C47-9F63-5A1D8E2F7C10}
AppName=Switchdesk
AppVersion={#AppVersion}
AppPublisher=m-dan12
AppPublisherURL=https://github.com/m-dan12/switchdesk
DefaultDirName={autopf}\Switchdesk
DisableProgramGroupPage=yes
DisableDirPage=auto
; Установка для текущего пользователя без UAC — так обновления проходят тихо.
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\dist
OutputBaseFilename=switchdesk-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes
RestartApplications=no
UninstallDisplayName=Switchdesk
UninstallDisplayIcon={app}\switchdesk.exe

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Tasks]
Name: "desktopicon"; Description: "Создать ярлык на рабочем столе"; Flags: unchecked

[InstallDelete]
; остатки версии 0.1 (два отдельных приложения)
Type: files; Name: "{app}\switchdesk-client.exe"
Type: files; Name: "{app}\switchdesk-host.exe"
Type: files; Name: "{autoprograms}\Switchdesk клиент.lnk"
Type: files; Name: "{autoprograms}\Switchdesk хост.lnk"
Type: files; Name: "{userstartup}\Switchdesk клиент.lnk"
Type: files; Name: "{userstartup}\Switchdesk хост.lnk"

[Files]
Source: "..\target\release\switchdesk.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Switchdesk"; Filename: "{app}\switchdesk.exe"
Name: "{autodesktop}\Switchdesk"; Filename: "{app}\switchdesk.exe"; Tasks: desktopicon

[Registry]
; автозапуск включается самим приложением (Настройки → «Запускать при входе в Windows»);
; при удалении программы убираем запись
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: none; ValueName: "Switchdesk"; Flags: dontcreatekey uninsdeletevalue

[Run]
Filename: "{app}\switchdesk.exe"; Description: "Запустить Switchdesk"; Flags: nowait postinstall skipifsilent

[UninstallRun]
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM switchdesk.exe"; Flags: runhidden; RunOnceId: "StopSwitchdesk"
