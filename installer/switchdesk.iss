; Установщик Switchdesk (Inno Setup 6). Сборка:
;   iscc /DAppVersion=0.1.0 installer\switchdesk.iss
; Ожидает собранные exe в target\release.

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
; По умолчанию — установка для текущего пользователя без UAC (так обновления тихие).
; Для установки на все профили выберите режим «для всех пользователей» в начале мастера.
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

[Languages]
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[Types]
Name: "client"; Description: "Только клиент (ваш компьютер)"
Name: "host"; Description: "Только хост (общий компьютер)"
Name: "full"; Description: "Клиент и хост"
Name: "custom"; Description: "Выборочная установка"; Flags: iscustom

[Components]
Name: "client"; Description: "Клиент (перехват клавиатуры и мыши)"; Types: client full custom
Name: "host"; Description: "Хост (приём ввода)"; Types: host full custom

[Tasks]
Name: "autostart"; Description: "Запускать при входе в Windows"; Flags: unchecked

[Files]
Source: "..\target\release\switchdesk-client.exe"; DestDir: "{app}"; Components: client; Flags: ignoreversion
Source: "..\target\release\switchdesk-host.exe"; DestDir: "{app}"; Components: host; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Switchdesk клиент"; Filename: "{app}\switchdesk-client.exe"; Parameters: "{code:ClientParams}"; Components: client; Check: ClientConfigured
Name: "{autoprograms}\Switchdesk хост"; Filename: "{app}\switchdesk-host.exe"; Parameters: "{code:HostParams}"; Components: host; Check: HostConfigured
Name: "{userstartup}\Switchdesk клиент"; Filename: "{app}\switchdesk-client.exe"; Parameters: "{code:ClientParams}"; Components: client; Tasks: autostart; Check: ClientConfigured; Flags: runminimized
Name: "{userstartup}\Switchdesk хост"; Filename: "{app}\switchdesk-host.exe"; Parameters: "{code:HostParams}"; Components: host; Tasks: autostart; Check: HostConfigured; Flags: runminimized

[Run]
Filename: "{app}\switchdesk-client.exe"; Parameters: "{code:ClientParams}"; Description: "Запустить клиент"; Components: client; Check: ClientConfigured; Flags: nowait postinstall skipifsilent
Filename: "{app}\switchdesk-host.exe"; Parameters: "{code:HostParams}"; Description: "Запустить хост"; Components: host; Check: HostConfigured; Flags: nowait postinstall skipifsilent unchecked

[Code]
var
  ClientPage, HostPage: TInputQueryWizardPage;

function Clean(S: String): String;
begin
  StringChangeEx(S, '"', '', True);
  Result := Trim(S);
end;

procedure InitializeWizard;
begin
  ClientPage := CreateInputQueryPage(wpSelectComponents, 'Клиент',
    'К какому хосту подключаться', 'Адрес хоста в Tailscale и имя, под которым вас увидят.');
  ClientPage.Add('Адрес хоста (например, server.tailnet.ts.net:7878):', False);
  ClientPage.Add('Ваше имя:', False);
  ClientPage.Values[0] := GetPreviousData('ClientHost', '');
  ClientPage.Values[1] := GetPreviousData('ClientName', GetComputerNameString);

  HostPage := CreateInputQueryPage(ClientPage.ID, 'Хост',
    'На каком адресе принимать ввод', 'Укажите адрес Tailscale этого компьютера (100.x.y.z:7878), не 0.0.0.0.');
  HostPage.Add('Адрес прослушивания:', False);
  HostPage.Values[0] := GetPreviousData('HostBind', '');
end;

procedure RegisterPreviousData(PreviousDataKey: Integer);
begin
  SetPreviousData(PreviousDataKey, 'ClientHost', Clean(ClientPage.Values[0]));
  SetPreviousData(PreviousDataKey, 'ClientName', Clean(ClientPage.Values[1]));
  SetPreviousData(PreviousDataKey, 'HostBind', Clean(HostPage.Values[0]));
end;

function ShouldSkipPage(PageID: Integer): Boolean;
begin
  Result := False;
  if PageID = ClientPage.ID then
    Result := not WizardIsComponentSelected('client');
  if PageID = HostPage.ID then
    Result := not WizardIsComponentSelected('host');
end;

function ClientConfigured: Boolean;
begin
  Result := Clean(ClientPage.Values[0]) <> '';
end;

function HostConfigured: Boolean;
begin
  Result := Clean(HostPage.Values[0]) <> '';
end;

function ClientParams(Param: String): String;
var
  Name: String;
begin
  Name := Clean(ClientPage.Values[1]);
  if Name = '' then Name := 'client';
  Result := '--host "' + Clean(ClientPage.Values[0]) + '" --name "' + Name + '"';
end;

function HostParams(Param: String): String;
begin
  Result := '--bind "' + Clean(HostPage.Values[0]) + '"';
end;
