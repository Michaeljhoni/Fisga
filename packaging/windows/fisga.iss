; Fisga's Windows installer (Inno Setup 6), from the folder packaging/windows/bundle.sh
; lays out. Per user, no administrator: %LOCALAPPDATA%\Programs\Fisga.
;   iscc /DAppVersion=1.0.0 packaging\windows\fisga.iss   -> dist\windows\Fisga-1.0.0-setup.exe

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif

[Setup]
AppId={{D8B18FC7-B467-4B22-92E1-B61CFC2AA354}
AppName=Fisga
AppVersion={#AppVersion}
AppVerName=Fisga {#AppVersion}
AppPublisher=Fisga
AppPublisherURL=https://github.com/Michaeljhoni/Fisga
AppSupportURL=https://github.com/Michaeljhoni/Fisga/issues
DefaultDirName={autopf}\Fisga
DisableProgramGroupPage=yes
DisableDirPage=auto
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
LicenseFile=..\..\LICENSE
SetupIconFile=fisga.ico
UninstallDisplayIcon={app}\bin\fisga-gui.exe
UninstallDisplayName=Fisga
OutputDir=..\..\dist\windows
OutputBaseFilename=Fisga-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Languages]
Name: "pt_BR"; MessagesFile: "compiler:Languages\BrazilianPortuguese.isl"
Name: "en"; MessagesFile: "compiler:Default.isl"

[CustomMessages]
pt_BR.Autostart=Iniciar o Fisga com o Windows (na bandeja, para pegar os downloads do navegador)
en.Autostart=Start Fisga with Windows (in the tray, to catch the browser's downloads)
pt_BR.Hosts=Ligando o Fisga aos navegadores...
en.Hosts=Connecting Fisga to the browsers...
pt_BR.Firefox=Instalar a extensão do Fisga no Firefox
en.Firefox=Install Fisga's extension in Firefox
pt_BR.ChromeGuide=Abrir o Fisga e ver como instalar a extensão no Chrome
en.ChromeGuide=Open Fisga and see how to install the extension in Chrome

[Tasks]
Name: "autostart"; Description: "{cm:Autostart}"
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\..\dist\windows\Fisga\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs

[InstallDelete]
; GTK and its DLLs change between versions: no leftovers from the previous one.
Type: filesandordirs; Name: "{app}\bin"
Type: filesandordirs; Name: "{app}\lib"
Type: filesandordirs; Name: "{app}\share"

[Icons]
; The AppUserModelID is the one notifications and the taskbar group by.
Name: "{autoprograms}\Fisga"; Filename: "{app}\bin\fisga-gui.exe"; AppUserModelID: "io.fisga.Fisga"
Name: "{autodesktop}\Fisga"; Filename: "{app}\bin\fisga-gui.exe"; AppUserModelID: "io.fisga.Fisga"; Tasks: desktopicon

[Registry]
; The same value the "Iniciar com a sessão" preference turns on and off.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Fisga"; ValueData: """{app}\bin\fisga-gui.exe"" --background"; Tasks: autostart; Flags: uninsdeletevalue

[Run]
Filename: "{app}\bin\fisga.exe"; Parameters: "install-hosts"; Flags: runhidden; StatusMsg: "{cm:Hosts}"
Filename: "https://addons.mozilla.org/firefox/addon/fisga@fisga.local/"; Description: "{cm:Firefox}"; Flags: postinstall shellexec skipifsilent; Check: FirefoxInstalled
; With Chrome, Brave or Vivaldi, launching Fisga also shows how to load its
; extension there (the Chrome Web Store does not carry it). One launch only:
; the two entries never show together.
Filename: "{app}\bin\fisga-gui.exe"; Parameters: "--chrome-guide"; Description: "{cm:ChromeGuide}"; Flags: postinstall nowait skipifsilent; Check: ChromiumInstalled
Filename: "{app}\bin\fisga-gui.exe"; Description: "{cm:LaunchProgram,Fisga}"; Flags: postinstall nowait skipifsilent; Check: not ChromiumInstalled
; An update Fisga installed itself (fisgad's update.rs): silent, and Fisga opens again.
Filename: "{app}\bin\fisga-gui.exe"; Flags: nowait; Check: Relaunch

[UninstallDelete]
; The video programs Fisga fetched (see tools.rs in the daemon), and its updates.
Type: filesandordirs; Name: "{localappdata}\fisga\tools"
Type: filesandordirs; Name: "{localappdata}\fisga\update"
; The daemon's pipe name (fisga-ipc's runtime_dir) and the browsers' manifests
; (hosts.rs). The settings, logins and download list, in {userappdata}\fisga, stay.
Type: filesandordirs; Name: "{localappdata}\fisga\run"
Type: filesandordirs; Name: "{localappdata}\fisga\native-messaging-hosts"
Type: dirifempty; Name: "{localappdata}\fisga"

[UninstallRun]
Filename: "{app}\bin\fisga.exe"; Parameters: "install-hosts --uninstall"; Flags: runhidden; RunOnceId: "Hosts"
Filename: "{app}\bin\fisga.exe"; Parameters: "stop"; Flags: runhidden; RunOnceId: "StopDaemon"
; `fisga stop` waits up to 10 s; a daemon still running would keep its files.
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM fisgad.exe"; Flags: runhidden; RunOnceId: "KillDaemon"
Filename: "{sys}\taskkill.exe"; Parameters: "/F /IM fisga-gui.exe"; Flags: runhidden; RunOnceId: "StopGui"

[Code]
function FirefoxInstalled: Boolean;
begin
  Result := RegKeyExists(HKLM, 'SOFTWARE\Mozilla\Mozilla Firefox') or RegKeyExists(HKCU, 'SOFTWARE\Mozilla\Mozilla Firefox');
end;

{ Chrome, Brave or Vivaldi (Edge comes with Windows, and does not count). }
function ChromiumInstalled: Boolean;
var
  I: Integer;
  Key: String;
begin
  Result := False;
  for I := 0 to 2 do
  begin
    case I of
      0: Key := 'chrome.exe';
      1: Key := 'brave.exe';
    else
      Key := 'vivaldi.exe';
    end;
    Key := 'SOFTWARE\Microsoft\Windows\CurrentVersion\App Paths\' + Key;
    if RegKeyExists(HKLM, Key) or RegKeyExists(HKCU, Key) then
      Result := True;
  end;
end;

{ /relaunch=yes: started by Fisga to update itself, which it closed. }
function Relaunch: Boolean;
begin
  Result := WizardSilent and (CompareText(ExpandConstant('{param:relaunch|no}'), 'yes') = 0);
end;

{ An update replaces the programs: the daemon pauses its downloads and quits
  (they resume when it starts again), the interface closes. `fisga stop`
  waits up to 10 s for the daemon; one still running is ended, or its
  program could not be replaced. }
function PrepareToInstall(var NeedsRestart: Boolean): String;
var
  Code: Integer;
begin
  if FileExists(ExpandConstant('{app}\bin\fisga.exe')) then
    Exec(ExpandConstant('{app}\bin\fisga.exe'), 'stop', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM fisgad.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Exec(ExpandConstant('{sys}\taskkill.exe'), '/F /IM fisga-gui.exe', '', SW_HIDE, ewWaitUntilTerminated, Code);
  Result := '';
end;
