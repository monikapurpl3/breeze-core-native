; Breeze Core - Windows guided installer (NSIS).
;
; Installs one executable and registers the hardened "BreezeCore" Windows
; service (see install-service.ps1). Two ways through it:
;
;   Simple    recommended settings; the only question is whether to set up
;             Caddy for public HTTPS at the end
;   Advanced  the install folder, the port, who can reach the server, its
;             firewall rules, the update check, Caddy, and extra environment
;             variables for the server
;
; and on an upgrade, "keep the current settings" or "review and change them".
;
; Two helpers are downloaded while it runs rather than packed into it, and
; each must match a pinned SHA-256 (install-service.ps1 holds both pins):
;
;   NSSM  runs the server as a service. Antivirus products flag installers that
;         carry it - malware uses it to persist - so it comes from nssm.cc, or
;         aspic's mirror. An upgrade reuses the installed copy, and an offline
;         machine can have nssm-2.24.zip put beside the installer.
;   GUP   the updater: WinGUp (LGPL-3.0), built from source with the Notepad++
;         branding patched out; see wingup\README.md. Optional - setup carries
;         on without it.
;
; Build: .\build-installer.ps1 (reads the version from Cargo.toml).
; Output: Breeze-Core-Setup.exe

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "nsDialogs.nsh"
!include "Sections.nsh"
!include "WinMessages.nsh"

!ifndef VERSION
  ; Deliberately NOT a real version. In the Python line this defaulted to
  ; "2.3.0", so an installer built without /DVERSION silently claimed to be
  ; 2.3.0 whatever source it actually contained - and one shipped that way.
  ; build-installer.ps1 reads the real version from Cargo.toml.
  !define VERSION "0.0.0-UNSET"
!endif

; The executable's size for the component description, measured by
; build-installer.ps1 from the binary it wraps. It used to be typed in, and
; said 2.2 MB for a while after it stopped being true.
!ifndef EXE_MB
  !error "EXE_MB is not defined - build with build-installer.ps1, which measures it"
!endif

; The updater's published name and checksum, from wingup\SHA256SUMS - the
; committed pin build-repo.sh also checks before publishing it.
!ifndef GUP_NAME | GUP_SHA256
  !error "GUP_NAME / GUP_SHA256 are not defined - build with build-installer.ps1, which reads them from wingup\SHA256SUMS"
!endif

; Where build-installer.ps1 stages breeze-core.exe, and the files it renders
; for this version (gup.xml, the updater's README). Overridable so a build from
; a different output directory does not need this file edited.
!ifndef BINDIR
  !define BINDIR "..\out\bin\windows"
!endif
!ifndef STAGE
  !define STAGE "..\out\windows-stage"
!endif

; Everything the updater may download starts with this. GUP enforces it
; (-forceDomain) whatever the feed says.
!define UPDATE_PREFIX "https://aspic.salataputarica.hr.eu.org/windows/"
!define GUP_URL "${UPDATE_PREFIX}updater/${GUP_NAME}"

; LZMA, solid. The payload is one already-optimised executable, and zlib (the
; default) left 250 KB on the table for a project whose headline number is its
; size.
SetCompressor /SOLID lzma

Name "Breeze Core ${VERSION}"
OutFile "Breeze-Core-Setup.exe"
InstallDir "$PROGRAMFILES64\Breeze Core"
; No InstallDirRegKey: it is read before .onInit, in the 32-bit registry view,
; and this installer writes its keys in the 64-bit one -- so it never found an
; earlier install, and every upgrade defaulted back to Program Files. .onInit
; reads the key itself, after SetRegView 64.
RequestExecutionLevel admin
ShowInstDetails show
ShowUnInstDetails show

Var RunWizard
Var PS
; "1" when a BreezeCore service running breeze-core.exe already exists: this
; run is an upgrade. A Python-era service is not one - it is replaced, and set
; up like a first install.
Var Upgrading
; What install-service.ps1 -Action Stop reported: 20 no service yet, 0 it was
; stopped, 10 it was running (stopped now, started again afterwards).
Var ServiceState
; simple | advanced on a first install, keep | change on an upgrade.
Var Mode
; The server settings: prefilled from the running service on an upgrade.
Var Port
Var LocalOnly
Var Firewall
Var Egress
Var BehindProxy
Var OldVersion
Var Summary
Var HaveGup
; Dialog controls.
Var Dlg
Var hRadio1
Var hRadio2
Var hCaddy
Var hPort
Var hLan
Var hLocal
Var hFw
Var hEgress
Var hEnv
Var hMono

; ----- MUI -----
!define MUI_ABORTWARNING
!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

; Said up front, because it is the one thing about this installer that is not
; obvious and would otherwise surface as an error halfway through.
!define MUI_WELCOMEPAGE_TEXT "Setup will install Breeze Core ${VERSION}, the LAN-first server for Midea air conditioners, as a Windows service.$\r$\n$\r$\nIt downloads two small helpers while it runs, each checked against a fixed checksum: NSSM, which runs the server as a service, and the updater. They are not packed into this installer because antivirus programs flag installers that carry NSSM.$\r$\n$\r$\nClick Next to continue."
!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
Page custom modeShow modeLeave
!define MUI_PAGE_CUSTOMFUNCTION_PRE componentsPre
!insertmacro MUI_PAGE_COMPONENTS
; Skipped on an upgrade: the service already runs from somewhere, and a second
; copy elsewhere would leave it running the old one.
!define MUI_PAGE_CUSTOMFUNCTION_PRE dirPre
!insertmacro MUI_PAGE_DIRECTORY
Page custom serverShow serverLeave
Page custom envShow envLeave
!insertmacro MUI_PAGE_INSTFILES

; Finish page: two optional next steps.
!define MUI_FINISHPAGE_RUN "$INSTDIR\pair.cmd"
!define MUI_FINISHPAGE_RUN_TEXT "Pair my AC units now (writes config.json)"
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Set up the Caddy reverse proxy now (for public HTTPS)"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION RunCaddyWizard
!define MUI_FINISHPAGE_LINK "Breeze Core on GitHub"
!define MUI_FINISHPAGE_LINK_LOCATION "https://github.com/monikapurpl3/breeze-core-native"
!define MUI_PAGE_CUSTOMFUNCTION_SHOW finishShow
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; One place for the helper's command line.
!define SVC '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$PLUGINSDIR\install-service.ps1"'

; ---------------------------------------------------------------- Sections
Section "Breeze Core server (Windows service)" SEC_SERVER
  SectionIn RO

  ; The helpers first, before anything is stopped or replaced: a machine that
  ; cannot get NSSM is left exactly as it was.
  DetailPrint "Getting NSSM (checked against a pinned SHA-256)..."
  nsExec::ExecToLog '${SVC} -Action FetchNssm -InstallDir "$INSTDIR" -LocalDir "$EXEDIR" -Dest "$PLUGINSDIR\nssm.exe"'
  Pop $0
  ${If} $0 != "0"
    MessageBox MB_ICONSTOP "Setup could not get NSSM, which runs Breeze Core as a service, so nothing has been installed or changed.$\n$\nConnect this PC to the internet and run setup again. Or download nssm-2.24.zip from https://nssm.cc/download, put it next to this installer, and run it again - it is checked against the same checksum.$\n$\nDetails are in the log above." /SD IDOK
    Abort
  ${EndIf}

  StrCpy $HaveGup "0"
  DetailPrint "Getting the updater (checked against a pinned SHA-256)..."
  nsExec::ExecToLog '${SVC} -Action FetchGup -InstallDir "$INSTDIR" -LocalDir "$EXEDIR" -Dest "$PLUGINSDIR\GUP.exe" -GupUrl "${GUP_URL}" -GupSha256 ${GUP_SHA256}'
  Pop $0
  ${If} $0 == "0"
    StrCpy $HaveGup "1"
  ${Else}
    DetailPrint "No updater this time. Breeze Core works without it; run setup again later to add it."
  ${EndIf}

  ; An existing service is stopped before anything is replaced: NSSM and
  ; breeze-core.exe are both locked while it runs. The helper comes from this
  ; installer, not the installed copy -- an older install-service.ps1 has no
  ; Stop action.
  nsExec::ExecToLog '${SVC} -Action Stop'
  Pop $ServiceState
  ${If} $ServiceState != "0"
  ${AndIf} $ServiceState != "10"
  ${AndIf} $ServiceState != "20"
    MessageBox MB_ICONSTOP "The BreezeCore service could not be stopped (result: $ServiceState), so its files cannot be replaced.$\n$\nStop it from Services, then run this installer again. Nothing has been changed." /SD IDOK
    Abort
  ${EndIf}

  SetOutPath "$INSTDIR"

  ; The server: one file, panel included.
  File "${BINDIR}\breeze-core.exe"

  ; Windows helper scripts.
  File "install-service.ps1"
  File "caddy-wizard.ps1"
  File "breeze-tripwire.ps1"
  File "Caddyfile.example"
  File "pair.cmd"
  File "README.md"
  File "..\..\LICENSE"
  ClearErrors
  CopyFiles /SILENT "$PLUGINSDIR\nssm.exe" "$INSTDIR\nssm.exe"
  ${If} ${Errors}
    MessageBox MB_ICONSTOP "Could not copy NSSM into $INSTDIR." /SD IDOK
    Abort
  ${EndIf}

  ; The updater, beside its own configuration and licence. gup.xml is
  ; rewritten on every install: it carries the version, and so the feed GUP
  ; asks about.
  ${If} $HaveGup == "1"
    CreateDirectory "$INSTDIR\updater"
    CopyFiles /SILENT "$PLUGINSDIR\GUP.exe" "$INSTDIR\updater\GUP.exe"
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\updater\GUP.exe"
    SetOutPath "$INSTDIR\updater"
    File "${STAGE}\gup.xml"
    File "${STAGE}\README.txt"
    File "wingup\nativeLang.xml"
    File "wingup\notices\LGPL-3.0.txt"
    File "wingup\notices\GPL-3.0.txt"
    File "wingup\notices\THIRD-PARTY-NOTICES.txt"
    SetOutPath "$INSTDIR"
  ${EndIf}

  WriteRegStr HKLM "Software\BreezeCore" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\BreezeCore" "Version" "${VERSION}"

  ; From here the installed copy of the helper is the one to use: it is this
  ; version's, and it is where the service's own shortcuts point.
  ${If} $Mode == "keep"
    ; Keep the bind address, port, proxy mode, environment and firewall rules
    ; exactly as they were -- re-registering from defaults used to undo all of
    ; them -- and start it again only if it was running.
    DetailPrint "Upgrading the BreezeCore service in place..."
    StrCpy $1 ""
    ${If} $ServiceState == "10"
      StrCpy $1 "-Start"
    ${EndIf}
    nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-service.ps1" -Action Upgrade -InstallDir "$INSTDIR" -Nssm "$INSTDIR\nssm.exe" $1'
  ${Else}
    ; A first install, a Python-era service being replaced, or an upgrade
    ; whose settings were reviewed: every setting is given explicitly.
    DetailPrint "Setting up the BreezeCore service..."
    StrCpy $1 "-Port $Port"
    ${If} $LocalOnly == "1"
      StrCpy $1 "$1 -BindHost 127.0.0.1"
      ; A server Caddy fronts must keep reading the client address from
      ; Caddy, or every proxied request looks like it came from this PC.
      ${If} $BehindProxy == "1"
        StrCpy $1 "$1 -BehindProxy"
      ${EndIf}
    ${EndIf}
    ${If} $Firewall != "1"
      StrCpy $1 "$1 -NoFirewall"
    ${EndIf}
    ${If} $Egress == "1"
      StrCpy $1 "$1 -LockEgress"
    ${EndIf}
    ${If} $Mode != "simple"
    ${AndIf} ${FileExists} "$PLUGINSDIR\service.env"
      StrCpy $1 '$1 -EnvFile "$PLUGINSDIR\service.env"'
    ${EndIf}
    ; Stopped on purpose before this run: leave it that way.
    ${If} $ServiceState == "0"
      StrCpy $1 "$1 -NoStart"
    ${EndIf}
    nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-service.ps1" -Action Install -InstallDir "$INSTDIR" -Nssm "$INSTDIR\nssm.exe" $1'
  ${EndIf}
  Pop $0
  ${If} $0 != 0
    ; $\" is how NSIS escapes a quote inside a string. A backtick does NOT
    ; escape anything here -- it opens a third kind of quoted string, which is
    ; how this line first turned into eleven arguments.
    MessageBox MB_ICONEXCLAMATION "Service setup returned code $0.$\n$\nThe files are installed; only the service registration failed. Run this from an elevated PowerShell to see why:$\n$\n    powershell -ExecutionPolicy Bypass -File $\"$INSTDIR\install-service.ps1$\" -Action Install$\n$\nDetails are in the install log above." /SD IDOK
  ${EndIf}

  ; Start-menu shortcuts.
  CreateDirectory "$SMPROGRAMS\Breeze Core"
  CreateShortcut "$SMPROGRAMS\Breeze Core\Pair AC units.lnk" "$INSTDIR\pair.cmd"
  CreateShortcut "$SMPROGRAMS\Breeze Core\Set up Caddy reverse proxy.lnk" "$PS" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\caddy-wizard.ps1"'
  CreateShortcut "$SMPROGRAMS\Breeze Core\Diagnose.lnk" "$INSTDIR\breeze-core.exe" 'diag'
  CreateShortcut "$SMPROGRAMS\Breeze Core\Edit service (nssm).lnk" "$INSTDIR\nssm.exe" 'edit BreezeCore'
  ${If} ${FileExists} "$INSTDIR\updater\GUP.exe"
    ; -verbose also says "up to date"; the sign-in check stays quiet then.
    SetOutPath "$INSTDIR\updater"
    CreateShortcut "$SMPROGRAMS\Breeze Core\Check for updates.lnk" "$INSTDIR\updater\GUP.exe" '-verbose -forceDomain=${UPDATE_PREFIX}'
    SetOutPath "$INSTDIR"
  ${Else}
    Delete "$SMPROGRAMS\Breeze Core\Check for updates.lnk"
  ${EndIf}
  CreateShortcut "$SMPROGRAMS\Breeze Core\Uninstall.lnk" "$INSTDIR\uninstall.exe"

  ; Uninstall registration.
  WriteUninstaller "$INSTDIR\uninstall.exe"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "DisplayName" "Breeze Core"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "Publisher" "Breeze Core"
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "InstallLocation" "$INSTDIR"
  ; Rough, and rough is the point: a wrong-by-a-megabyte figure in Programs and
  ; Features is better than a blank one.
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "EstimatedSize" 4700
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "NoRepair" 1
SectionEnd

; The check at sign-in. Nothing to copy: the server section installs the
; updater either way, for the Start-menu shortcut. UpdateCheck records the
; choice, so that a later "keep the current settings" can tell "turned off"
; from "never offered".
Section "Check for updates when I sign in" SEC_UPDATES
  ${If} ${FileExists} "$INSTDIR\updater\GUP.exe"
    nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-service.ps1" -Action RegisterUpdateCheck -InstallDir "$INSTDIR"'
    Pop $0
    WriteRegStr HKLM "Software\BreezeCore" "UpdateCheck" "1"
  ${Else}
    ; Wanted, but there is no updater yet: offer it again next time.
    DeleteRegValue HKLM "Software\BreezeCore" "UpdateCheck"
  ${EndIf}
SectionEnd

Section /o "Caddy reverse proxy (guided setup)" SEC_CADDY
  ; Nothing to copy (the wizard ships with the server component); this just
  ; opts you into running the guided Caddy wizard on the final page.
  StrCpy $RunWizard 1
SectionEnd

; Hidden, and last: what an unselected component has to undo.
Section "-post"
  ${IfNot} ${SectionIsSelected} ${SEC_UPDATES}
    nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-service.ps1" -Action UnregisterUpdateCheck'
    Pop $0
    WriteRegStr HKLM "Software\BreezeCore" "UpdateCheck" "0"
  ${EndIf}
SectionEnd

; Component descriptions.
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SERVER} "The Breeze Core server - one ${EXE_MB} MB executable with the web panel compiled in - run as a hardened Windows service: low-privilege account, firewall opened to the local network only. NSSM, which runs it as a service, is downloaded during setup. Required."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_UPDATES} "A few minutes after an administrator signs in, ask aspic whether a newer Breeze Core is out. Silent when there is not, and asks before installing anything. The Start menu gets a Check for updates shortcut either way."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CADDY} "Optional: run the guided Caddy reverse-proxy wizard at the end for public HTTPS (automatic TLS, hardened headers, LAN-only admin, fail2ban-style banning). You can also run it later from the Start menu."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------------------------------------------------------- Functions
Function .onInit
  StrCpy $RunWizard 0
  StrCpy $PS "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"

  ; makensis produces a 32-bit installer, so HKLM\Software writes are redirected
  ; into WOW6432Node unless told otherwise - the first install of this put both
  ; its keys there while installing a 64-bit binary into $PROGRAMFILES64.
  SetRegView 64

  ; And NSIS defaults the shell folders to the CURRENT USER, so a machine-wide
  ; install put its Start-menu shortcuts in the profile of whoever happened to
  ; run the installer. This is a service in Program Files; the shortcuts belong
  ; to all users.
  SetShellVarContext all

  ; Where an earlier install put itself, now that the view is right.
  ReadRegStr $0 HKLM "Software\BreezeCore" "InstallDir"
  ${If} $0 != ""
    StrCpy $INSTDIR $0
  ${EndIf}
  ReadRegStr $OldVersion HKLM "Software\BreezeCore" "Version"

  ; This version's helper, for the pages as well as the sections.
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "install-service.ps1"

  ; First-install defaults: what Simple installs.
  StrCpy $Mode "simple"
  StrCpy $Port "8420"
  StrCpy $LocalOnly "0"
  StrCpy $Firewall "1"
  StrCpy $Egress "0"
  StrCpy $BehindProxy "0"
  StrCpy $Upgrading ""

  ; An upgrade is decided by the service, not by our own keys: a service is
  ; what an upgrade must not break. Its settings are read back from the
  ; service itself, so "review and change" starts from what is really there.
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\BreezeCore" "ImagePath"
  ${If} $0 != ""
    StrCpy $Upgrading "1"
    nsExec::ExecToStack '${SVC} -Action Describe -OutDir "$PLUGINSDIR\now"'
    Pop $0
    Pop $1
    ${If} $0 == "0"
      ReadINIStr $0 "$PLUGINSDIR\now\service.ini" service native
      ${If} $0 == "0"
        ; A Python-era service: nothing of its setup carries over.
        StrCpy $Upgrading ""
      ${Else}
        ReadINIStr $Port "$PLUGINSDIR\now\service.ini" service port
        ReadINIStr $LocalOnly "$PLUGINSDIR\now\service.ini" service local
        ReadINIStr $Firewall "$PLUGINSDIR\now\service.ini" service firewall
        ReadINIStr $Egress "$PLUGINSDIR\now\service.ini" service egress
        ReadINIStr $BehindProxy "$PLUGINSDIR\now\service.ini" service behind_proxy
        CopyFiles /SILENT "$PLUGINSDIR\now\service.env" "$PLUGINSDIR\service.env"
        Call describeSummary
        ; The update check: on if it is on now, and on if this install has
        ; never been asked (it predates the updater); off only if it was
        ; turned off.
        ReadINIStr $0 "$PLUGINSDIR\now\service.ini" service update_check
        ReadRegStr $1 HKLM "Software\BreezeCore" "UpdateCheck"
        ${If} $0 != "1"
        ${AndIf} $1 != ""
          !insertmacro UnselectSection ${SEC_UPDATES}
        ${EndIf}
      ${EndIf}
    ${EndIf}
    ${If} $Upgrading == "1"
      StrCpy $Mode "keep"
    ${EndIf}
  ${EndIf}

  ; An empty list offers the examples instead.
  ${GetSize} "$PLUGINSDIR" "/M=service.env /S=0B /G=0" $0 $1 $2
  ${If} $0 < 3
    Call writeEnvTemplate
  ${EndIf}
FunctionEnd

; One line saying what is set up now, for the upgrade page.
Function describeSummary
  ${If} $BehindProxy == "1"
    StrCpy $Summary "port $Port, behind Caddy (this PC only)"
  ${ElseIf} $LocalOnly == "1"
    StrCpy $Summary "port $Port, this PC only"
  ${Else}
    ReadINIStr $0 "$PLUGINSDIR\now\service.ini" service host
    StrCpy $Summary "port $Port, local network ($0)"
    ${If} $Firewall == "1"
      StrCpy $Summary "$Summary, firewall open to it"
    ${Else}
      StrCpy $Summary "$Summary, no firewall rule"
    ${EndIf}
  ${EndIf}
  ${If} $Egress == "1"
    StrCpy $Summary "$Summary, internet blocked"
  ${EndIf}
  ReadINIStr $0 "$PLUGINSDIR\now\service.ini" service env_lines
  ${If} $0 == "1"
    StrCpy $Summary "$Summary, 1 extra environment variable"
  ${ElseIf} $0 != "0"
  ${AndIf} $0 != ""
    StrCpy $Summary "$Summary, $0 extra environment variables"
  ${EndIf}
  ReadINIStr $0 "$PLUGINSDIR\now\service.ini" service update_check
  ${If} $0 == "1"
    StrCpy $Summary "$Summary, update check at sign-in"
  ${EndIf}
FunctionEnd

; ----- Simple / Advanced, or Keep / Change -----
Function modeShow
  nsDialogs::Create 1018
  Pop $Dlg
  ${If} $Upgrading == "1"
    ${If} $OldVersion != ""
      !insertmacro MUI_HEADER_TEXT "Upgrade Breeze Core" "Breeze Core $OldVersion is installed; this upgrades it to ${VERSION}."
    ${Else}
      !insertmacro MUI_HEADER_TEXT "Upgrade Breeze Core" "Breeze Core is installed; this upgrades it to ${VERSION}."
    ${EndIf}
    ${NSD_CreateLabel} 0 0 100% 20u "Now: $Summary."
    Pop $0
    ${NSD_CreateFirstRadioButton} 0 26u 100% 12u "&Keep the current settings (recommended)"
    Pop $hRadio1
    ${NSD_CreateLabel} 12u 39u -12u 20u "The port, network, firewall rules, environment and update check stay exactly as they are."
    Pop $0
    ${NSD_CreateRadioButton} 0 64u 100% 12u "&Review and change the settings"
    Pop $hRadio2
    ${NSD_CreateLabel} 12u 77u -12u 20u "Shows the current settings so you can change any of them."
    Pop $0
    ${NSD_CreateLabel} 0 108u 100% 20u "Either way your paired units, devices, programs and timers are kept."
    Pop $0
    ${If} $Mode == "change"
      ${NSD_Check} $hRadio2
    ${Else}
      ${NSD_Check} $hRadio1
    ${EndIf}
  ${Else}
    !insertmacro MUI_HEADER_TEXT "Choose how to install" "Simple uses the recommended settings. Advanced lets you choose them."
    ${NSD_CreateFirstRadioButton} 0 0 100% 12u "&Simple (recommended)"
    Pop $hRadio1
    ${NSD_CreateLabel} 12u 13u -12u 30u "Installs to Program Files. Phones and PCs on your network reach it on port 8420, and the firewall is opened to your network only. Checks for updates a few minutes after you sign in, and asks before installing any."
    Pop $0
    ${NSD_CreateCheckbox} 12u 46u -12u 12u "Also set up &Caddy for public HTTPS (asks for your domain at the end)"
    Pop $hCaddy
    ${NSD_CreateRadioButton} 0 68u 100% 12u "&Advanced"
    Pop $hRadio2
    ${NSD_CreateLabel} 12u 81u -12u 30u "Choose the install folder, the port, who can reach the server, its firewall rules, the update check, Caddy, and extra environment variables."
    Pop $0
    ${If} $Mode == "advanced"
      ${NSD_Check} $hRadio2
    ${Else}
      ${NSD_Check} $hRadio1
    ${EndIf}
    ${If} ${SectionIsSelected} ${SEC_CADDY}
      ${NSD_Check} $hCaddy
    ${EndIf}
    ${NSD_OnClick} $hRadio1 modeClick
    ${NSD_OnClick} $hRadio2 modeClick
    Push $hRadio1
    Call modeClick
  ${EndIf}
  nsDialogs::Show
FunctionEnd

; The Caddy box belongs to Simple; Advanced has it on the components page.
Function modeClick
  Pop $0
  ${NSD_GetState} $hRadio1 $0
  ${If} $0 == ${BST_CHECKED}
    EnableWindow $hCaddy 1
  ${Else}
    EnableWindow $hCaddy 0
  ${EndIf}
FunctionEnd

Function modeLeave
  ${NSD_GetState} $hRadio1 $0
  ${If} $Upgrading == "1"
    ${If} $0 == ${BST_CHECKED}
      StrCpy $Mode "keep"
      ; Caddy is already set up, or never wanted; the Start menu has it.
      !insertmacro UnselectSection ${SEC_CADDY}
    ${Else}
      StrCpy $Mode "change"
    ${EndIf}
  ${Else}
    ${If} $0 == ${BST_CHECKED}
      StrCpy $Mode "simple"
      ; Simple's settings, whatever an earlier trip through Advanced chose.
      StrCpy $INSTDIR "$PROGRAMFILES64\Breeze Core"
      StrCpy $Port "8420"
      StrCpy $LocalOnly "0"
      StrCpy $Firewall "1"
      StrCpy $Egress "0"
      !insertmacro SelectSection ${SEC_UPDATES}
      ${NSD_GetState} $hCaddy $1
      ${If} $1 == ${BST_CHECKED}
        !insertmacro SelectSection ${SEC_CADDY}
      ${Else}
        !insertmacro UnselectSection ${SEC_CADDY}
      ${EndIf}
    ${Else}
      StrCpy $Mode "advanced"
    ${EndIf}
  ${EndIf}
FunctionEnd

Function componentsPre
  ${If} $Mode == "simple"
  ${OrIf} $Mode == "keep"
    Abort
  ${EndIf}
FunctionEnd

Function dirPre
  ${If} $Upgrading == "1"
  ${OrIf} $Mode == "simple"
    Abort
  ${EndIf}
FunctionEnd

; ----- Server settings -----
Function serverShow
  ${If} $Mode == "simple"
  ${OrIf} $Mode == "keep"
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "Server settings" "The port, who can reach the server, and its firewall rules."
  nsDialogs::Create 1018
  Pop $Dlg

  ${NSD_CreateLabel} 0 2u 30u 12u "&Port:"
  Pop $0
  ${NSD_CreateNumber} 32u 0 36u 12u "$Port"
  Pop $hPort
  ${NSD_SetTextLimit} $hPort 5
  ${NSD_CreateLabel} 74u 2u -74u 12u "1 to 65535. 8420 unless something else already uses it."
  Pop $0

  ${NSD_CreateGroupBox} 0 18u 100% 44u "Who can reach the server"
  Pop $0
  ${NSD_CreateFirstRadioButton} 8u 30u -16u 12u "Phones, tablets and PCs on my &local network"
  Pop $hLan
  ${NSD_CreateRadioButton} 8u 44u -16u 12u "&Only this PC (127.0.0.1)"
  Pop $hLocal

  ${NSD_CreateCheckbox} 0 68u 100% 12u "Allow it through Windows &Firewall, from the local network only"
  Pop $hFw
  ${NSD_CreateCheckbox} 0 84u 100% 12u "&Block the server from the internet"
  Pop $hEgress
  ${NSD_CreateLabel} 12u 97u -12u 18u "Pairing a new unit needs the internet (Midea's cloud hands out its key), so pair your units first."
  Pop $0

  ${If} $BehindProxy == "1"
    ${NSD_CreateLabel} 0 118u 100% 22u "Caddy is in front of this server. Keep $\"Only this PC$\" unless you are removing Caddy: it reaches the server on 127.0.0.1."
    Pop $0
  ${ElseIf} ${SectionIsSelected} ${SEC_CADDY}
    ${NSD_CreateLabel} 0 118u 100% 22u "Caddy's setup, at the end, moves the server behind Caddy on this PC only. These settings apply until then, and if it is cancelled."
    Pop $0
  ${EndIf}

  ${If} $LocalOnly == "1"
    ${NSD_Check} $hLocal
  ${Else}
    ${NSD_Check} $hLan
  ${EndIf}
  ${If} $Firewall == "1"
    ${NSD_Check} $hFw
  ${EndIf}
  ${If} $Egress == "1"
    ${NSD_Check} $hEgress
  ${EndIf}
  ${NSD_OnClick} $hLan serverClick
  ${NSD_OnClick} $hLocal serverClick
  Push $hLan
  Call serverClick
  nsDialogs::Show
FunctionEnd

; A firewall rule for the LAN means nothing when only this PC can connect.
Function serverClick
  Pop $0
  ${NSD_GetState} $hLan $0
  ${If} $0 == ${BST_CHECKED}
    EnableWindow $hFw 1
  ${Else}
    EnableWindow $hFw 0
  ${EndIf}
FunctionEnd

Function serverLeave
  ${NSD_GetText} $hPort $0
  IntOp $1 $0 + 0
  ${If} $1 < 1
  ${OrIf} $1 > 65535
    MessageBox MB_ICONEXCLAMATION "The port must be a number from 1 to 65535."
    Abort
  ${EndIf}
  ${NSD_GetState} $hLocal $2
  ${NSD_GetState} $hFw $3
  ${NSD_GetState} $hEgress $4

  ; Something else listening there? A service that cannot bind restarts every
  ; five seconds, and the only sign of it is a log nobody has opened yet. Its
  ; own port does not count: the upgrade stops it first.
  nsExec::ExecToStack '${SVC} -Action CheckPort -Port $1'
  Pop $5
  Pop $6
  ${If} $5 == "1"
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "Port $1 is already in use, by $6.$\n$\nBreeze Core could not start while that is running. Use port $1 anyway?" IDYES portok
    Abort
  portok:
  ${EndIf}

  StrCpy $Port $1
  ${If} $2 == ${BST_CHECKED}
    StrCpy $LocalOnly "1"
  ${Else}
    StrCpy $LocalOnly "0"
  ${EndIf}
  ${If} $3 == ${BST_CHECKED}
    StrCpy $Firewall "1"
  ${Else}
    StrCpy $Firewall "0"
  ${EndIf}
  ${If} $4 == ${BST_CHECKED}
    StrCpy $Egress "1"
  ${Else}
    StrCpy $Egress "0"
  ${EndIf}
FunctionEnd

; ----- Environment variables -----
;
; The list lives in $PLUGINSDIR\service.env (UTF-16 LE) between pages, and
; goes to the edit box and back through the System plugin rather than
; ${NSD_GetText}: NSIS strings stop at 1024 characters, and a list longer
; than that would be cut off without a word.
Function envShow
  ${If} $Mode == "simple"
  ${OrIf} $Mode == "keep"
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "Environment variables" "Extra settings for the server, as its service will see them."
  nsDialogs::Create 1018
  Pop $Dlg
  ${NSD_CreateLabel} 0 0 100% 30u "One NAME=value per line; lines starting with # are ignored. The port, the network and the data folder come from the earlier pages and cannot be set here. Every setting is listed on the wiki's Configuration page."
  Pop $0
  ${NSD_CreateMLText} 0 34u 100% 104u ""
  Pop $hEnv
  ${If} $hMono == ""
    CreateFont $hMono "Consolas" 9
  ${EndIf}
  SendMessage $hEnv ${WM_SETFONT} $hMono 1
  SendMessage $hEnv ${EM_LIMITTEXT} 32768 0
  Call envLoad
  nsDialogs::Show
FunctionEnd

Function envLeave
  Call envSave
  nsExec::ExecToStack '${SVC} -Action CheckEnv -EnvFile "$PLUGINSDIR\service.env"'
  Pop $0
  Pop $1
  ${If} $0 != "0"
    MessageBox MB_ICONEXCLAMATION "$1"
    Abort
  ${EndIf}
FunctionEnd

Function writeEnvTemplate
  FileOpen $0 "$PLUGINSDIR\service.env" w
  FileWriteUTF16LE /BOM $0 "# Examples - remove the # at the start of a line to use it.$\r$\n"
  FileWriteUTF16LE $0 "#BREEZE_KEEP_WARM=30m$\r$\n"
  FileWriteUTF16LE $0 "#BREEZE_WORKERS=8$\r$\n"
  FileWriteUTF16LE $0 "#BREEZE_BG_WORKERS=1$\r$\n"
  FileClose $0
FunctionEnd

Function envLoad
  ClearErrors
  FileOpen $0 "$PLUGINSDIR\service.env" r
  ${If} ${Errors}
    Return
  ${EndIf}
  FileSeek $0 0 END $1
  FileSeek $0 0 SET
  ; Zero-filled, so the spare bytes terminate the string.
  IntOp $2 $1 + 4
  System::Alloc $2
  Pop $3
  System::Call 'kernel32::ReadFile(p r0, p r3, i r1, *i .r4, p 0)'
  FileClose $0
  System::Call '*$3(&i2 .r5)'
  IntOp $5 $5 & 0xFFFF
  ${If} $5 = 0xFEFF
    IntOp $6 $3 + 2
  ${Else}
    StrCpy $6 $3
  ${EndIf}
  System::Call 'user32::SetWindowTextW(p $hEnv, p r6)'
  System::Free $3
FunctionEnd

Function envSave
  System::Call 'user32::GetWindowTextLengthW(p $hEnv) i .r1'
  IntOp $2 $1 + 1
  IntOp $3 $2 * 2
  System::Alloc $3
  Pop $4
  System::Call 'user32::GetWindowTextW(p $hEnv, p r4, i r2) i .r5'
  FileOpen $0 "$PLUGINSDIR\service.env" w
  FileWriteWord $0 0xFEFF
  IntOp $6 $5 * 2
  System::Call 'kernel32::WriteFile(p r0, p r4, i r6, *i .r7, p 0)'
  FileClose $0
  System::Free $4
FunctionEnd

; Same two settings for the uninstaller, which is a separate process: without
; them it would look for its keys in the 64-bit view it never wrote to, and
; delete shortcuts from the wrong Start menu.
Function un.onInit
  SetRegView 64
  SetShellVarContext all
FunctionEnd

; Pre-check the "set up Caddy" finish checkbox iff the component was selected.
Function finishShow
  ${If} $RunWizard == 1
    SendMessage $mui.FinishPage.ShowReadme ${BM_SETCHECK} ${BST_CHECKED} 0
  ${EndIf}
  ; An upgrade keeps its pairing: offering to pair again by default would
  ; invite overwriting a working config.json.
  ${If} $Upgrading == "1"
    SendMessage $mui.FinishPage.Run ${BM_SETCHECK} ${BST_UNCHECKED} 0
  ${EndIf}
FunctionEnd

; Launch the Caddy wizard in its own elevated PowerShell window, pointed at
; the port the server was given here.
Function RunCaddyWizard
  Exec '"$PS" -NoProfile -ExecutionPolicy Bypass -NoExit -File "$INSTDIR\caddy-wizard.ps1" -Upstream 127.0.0.1:$Port'
FunctionEnd

; ---------------------------------------------------------------- Uninstaller
Section "Uninstall"
  StrCpy $PS "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"

  ; Remove the BreezeCore service, its firewall rules and the update check
  ; (keeps the data dir).
  nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-service.ps1" -Action Uninstall -InstallDir "$INSTDIR" -Nssm "$INSTDIR\nssm.exe"'
  Pop $0

  ; Remove Caddy + tripwire services and their firewall rules, if present.
  nsExec::ExecToLog '"$INSTDIR\nssm.exe" stop BreezeCaddy confirm'
  nsExec::ExecToLog '"$INSTDIR\nssm.exe" remove BreezeCaddy confirm'
  nsExec::ExecToLog '"$INSTDIR\nssm.exe" stop BreezeTripwire confirm'
  nsExec::ExecToLog '"$INSTDIR\nssm.exe" remove BreezeTripwire confirm'
  nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -Command "Get-NetFirewallRule -DisplayName ''Breeze *'' -ErrorAction SilentlyContinue | Remove-NetFirewallRule; Get-NetFirewallRule -DisplayName ''BreezeBan *'' -ErrorAction SilentlyContinue | Remove-NetFirewallRule"'

  Delete "$SMPROGRAMS\Breeze Core\*.lnk"
  RMDir "$SMPROGRAMS\Breeze Core"

  ; App files. The data dir under %ProgramData%\breeze-core is left alone.
  RMDir /r "$INSTDIR\caddy"
  RMDir /r "$INSTDIR\updater"
  Delete "$INSTDIR\breeze-core.exe"
  Delete "$INSTDIR\nssm.exe"
  Delete "$INSTDIR\*.ps1"
  Delete "$INSTDIR\*.cmd"
  Delete "$INSTDIR\*.md"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\Caddyfile.example"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"

  DeleteRegKey HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore"
  DeleteRegKey HKLM "Software\BreezeCore"

  MessageBox MB_OK "Breeze Core removed.$\n$\nYour configuration and device tokens were kept in:$\n    %ProgramData%\breeze-core$\n$\nA paired V3 unit's token and key cannot be obtained from Midea again, so that folder is worth keeping. Delete it by hand if you are sure." /SD IDOK
SectionEnd
