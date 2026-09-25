; packaging/windows/holter-http-api.iss — InnoBuilder (packaging-distribution)
;
; Windows x86_64 CPU-default installer. Copies the verified staging tree
; (embedded holter-http-api.exe + sample ini + NOTICE + short docs).
; Does NOT register a Windows Service. Does NOT produce MSI/WiX.
;
; Compile (Windows runner with Inno Setup 6.x / ISCC on PATH):
;   ./packaging/scripts/build-inno.sh \
;     --staging packaging/out/staging/windows \
;     --version 1.0.0
;
; Manual equivalent:
;   iscc /DMyAppVersion=1.0.0 /DStagingDir=..\out\staging\windows \
;        packaging\windows\holter-http-api.iss
;
; Output: holter-http-api-setup-<version>-cpu.exe
; Tag policy: CPU is the required default; CUDA (if any) is a separate artifact.
;
; After install: copy http.ini.example → config\http.ini, set license server_url
; and http bind, then run holter-http-api.exe --config config\http.ini
; (see docs/packaging/windows.md — InstallDocs task).

#ifndef MyAppVersion
  #define MyAppVersion "0.1.0"
#endif

#ifndef StagingDir
  ; Relative to this .iss file (packaging/windows/)
  #define StagingDir "..\out\staging\windows"
#endif

#ifndef OutputDir
  #define OutputDir "..\out\windows"
#endif

[Setup]
AppId={{A8F3C2E1-9B47-4D6A-8E21-5C7D0B1A4F32}}
AppName=Holter HTTP API
AppVersion={#MyAppVersion}
AppPublisher=Holter Analysis Assist
DefaultDirName={autopf}\Holter HTTP API
DefaultGroupName=Holter HTTP API
DisableProgramGroupPage=yes
OutputDir={#OutputDir}
OutputBaseFilename=holter-http-api-setup-{#MyAppVersion}-cpu
Compression=lzma
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
UninstallDisplayName=Holter HTTP API
; Fail closed: ISCC returns non-zero on compile errors (do not treat as success).

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Files]
; Staging layout (packaging/out/staging/windows): bin/, NOTICE, http.ini.example, docs/
Source: "{#StagingDir}\bin\holter-http-api.exe"; DestDir: "{app}\bin"; Flags: ignoreversion
Source: "{#StagingDir}\NOTICE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\http.ini.example"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#StagingDir}\docs\*"; DestDir: "{app}\docs"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{group}\Holter HTTP API"; Filename: "{app}\bin\holter-http-api.exe"
Name: "{group}\Uninstall Holter HTTP API"; Filename: "{uninstallexe}"

; No [Services] section — operators start the process manually after editing ini.
; No MSI / WiX output — Inno Setup EXE installer only.
