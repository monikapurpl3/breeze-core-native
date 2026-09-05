; Breeze Core - Windows guided installer (NSIS).
;
; Installs one executable plus the bundled NSSM and registers the hardened
; "BreezeCore" Windows service (see install-service.ps1). The Caddy
; reverse-proxy setup is a SEPARATE, optional component - the server installs
; and runs LAN-first without it; add Caddy only if exposing it publicly.
;
; **This installer needs no internet.** The Python line's version had to find a
; Python 3.11+, build a virtualenv and download dependencies, and most of its
; failures were one of those three going wrong on somebody else's machine.
; There is nothing to download here: the whole server is one file.
;
; Build:
;   powershell -ExecutionPolicy Bypass -File fetch-vendor.ps1   (gets vendor\nssm.exe)
;   .\build-installer.ps1                                       (reads the version from Cargo.toml)
; Output: Breeze-Core-Setup.exe

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"

!ifndef VERSION
  ; Deliberately NOT a real version. In the Python line this defaulted to
  ; "2.3.0", so an installer built without /DVERSION silently claimed to be
  ; 2.3.0 whatever source it actually contained - and one shipped that way.
  ; build-installer.ps1 reads the real version from Cargo.toml.
  !define VERSION "0.0.0-UNSET"
!endif

; Where build-installer.ps1 stages breeze-core.exe. Overridable so a build from
; a different output directory does not need this file edited.
!ifndef BINDIR
  !define BINDIR "..\out\bin\windows"
!endif

; LZMA, solid. The payload is one already-optimised executable, and zlib (the
; default) left 250 KB on the table for a project whose headline number is its
; size.
SetCompressor /SOLID lzma

Name "Breeze Core ${VERSION}"
OutFile "Breeze-Core-Setup.exe"
InstallDir "$PROGRAMFILES64\Breeze Core"
InstallDirRegKey HKLM "Software\BreezeCore" "InstallDir"
RequestExecutionLevel admin
ShowInstDetails show
ShowUnInstDetails show

Var RunWizard
Var PS

; ----- MUI -----
!define MUI_ABORTWARNING
!define MUI_ICON "${NSISDIR}\Contrib\Graphics\Icons\modern-install.ico"
!define MUI_UNICON "${NSISDIR}\Contrib\Graphics\Icons\modern-uninstall.ico"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES

; Finish page: two optional next steps.
!define MUI_FINISHPAGE_RUN "$INSTDIR\pair.cmd"
!define MUI_FINISHPAGE_RUN_TEXT "Pair my AC units now (writes config.json)"
!define MUI_FINISHPAGE_SHOWREADME ""
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Set up the Caddy reverse proxy now (for public HTTPS)"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED
!define MUI_FINISHPAGE_SHOWREADME_FUNCTION RunCaddyWizard
!define MUI_FINISHPAGE_LINK "Breeze Core on GitHub"
!define MUI_FINISHPAGE_LINK_LOCATION "https://github.com/monikapurpl3/breeze-core"
!define MUI_PAGE_CUSTOMFUNCTION_SHOW finishShow
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

; ---------------------------------------------------------------- Sections
Section "Breeze Core server (Windows service)" SEC_SERVER
  SectionIn RO
  SetOutPath "$INSTDIR"

  ; The server: one file, panel included.
  File "${BINDIR}\breeze-core.exe"

  ; Windows helper scripts + bundled NSSM.
  File "install-service.ps1"
  File "caddy-wizard.ps1"
  File "breeze-tripwire.ps1"
  File "Caddyfile.example"
  File "pair.cmd"
  File "README.md"
  File "..\..\LICENSE"
  File "vendor\nssm.exe"

  WriteRegStr HKLM "Software\BreezeCore" "InstallDir" "$INSTDIR"
  WriteRegStr HKLM "Software\BreezeCore" "Version" "${VERSION}"

  ; Register the hardened service (LAN-first bind, LOCAL SERVICE, locked-down
  ; %ProgramData%\breeze-core). No network access required.
  DetailPrint "Registering the BreezeCore service..."
  nsExec::ExecToLog '"$PS" -NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\install-service.ps1" -Action Install -InstallDir "$INSTDIR" -Nssm "$INSTDIR\nssm.exe"'
  Pop $0
  ${If} $0 != 0
    ; $\" is how NSIS escapes a quote inside a string. A backtick does NOT
    ; escape anything here -- it opens a third kind of quoted string, which is
    ; how this line first turned into eleven arguments.
    MessageBox MB_ICONEXCLAMATION "Service setup returned code $0.$\n$\nThe files are installed; only the service registration failed. Run this from an elevated PowerShell to see why:$\n$\n    powershell -ExecutionPolicy Bypass -File $\"$INSTDIR\install-service.ps1$\" -Action Install$\n$\nDetails are in the install log above."
  ${EndIf}

  ; Start-menu shortcuts.
  CreateDirectory "$SMPROGRAMS\Breeze Core"
  CreateShortcut "$SMPROGRAMS\Breeze Core\Pair AC units.lnk" "$INSTDIR\pair.cmd"
  CreateShortcut "$SMPROGRAMS\Breeze Core\Set up Caddy reverse proxy.lnk" "$PS" '-NoProfile -ExecutionPolicy Bypass -File "$INSTDIR\caddy-wizard.ps1"'
  CreateShortcut "$SMPROGRAMS\Breeze Core\Diagnose.lnk" "$INSTDIR\breeze-core.exe" 'diag'
  CreateShortcut "$SMPROGRAMS\Breeze Core\Edit service (nssm).lnk" "$INSTDIR\nssm.exe" 'edit BreezeCore'
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
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "EstimatedSize" 3200
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "NoModify" 1
  WriteRegDWORD HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\BreezeCore" "NoRepair" 1
SectionEnd

Section /o "Caddy reverse proxy (guided setup)" SEC_CADDY
  ; Nothing to copy (the wizard ships with the server component); this just
  ; opts you into running the guided Caddy wizard on the final page.
  StrCpy $RunWizard 1
SectionEnd

; Component descriptions.
!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_SERVER} "The Breeze Core server - one 2.2 MB executable with the web panel compiled in - run as a hardened Windows service (bundled NSSM, low-privilege account, LAN-locked firewall). Required."
  !insertmacro MUI_DESCRIPTION_TEXT ${SEC_CADDY} "Optional: run the guided Caddy reverse-proxy wizard at the end for public HTTPS (automatic TLS, hardened headers, LAN-only admin, fail2ban-style banning). You can also run it later from the Start menu."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

; ---------------------------------------------------------------- Functions
Function .onInit
  StrCpy $RunWizard 0
  StrCpy $PS "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"
FunctionEnd

; Pre-check the "set up Caddy" finish checkbox iff the component was selected.
Function finishShow
  ${If} $RunWizard == 1
    SendMessage $mui.FinishPage.ShowReadme ${BM_SETCHECK} ${BST_CHECKED} 0
  ${EndIf}
FunctionEnd

; Launch the Caddy wizard in its own elevated PowerShell window.
Function RunCaddyWizard
  Exec '"$PS" -NoProfile -ExecutionPolicy Bypass -NoExit -File "$INSTDIR\caddy-wizard.ps1"'
FunctionEnd

; ---------------------------------------------------------------- Uninstaller
Section "Uninstall"
  StrCpy $PS "$SYSDIR\WindowsPowerShell\v1.0\powershell.exe"

  ; Remove the BreezeCore service + its firewall rules (keeps the data dir).
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

  MessageBox MB_OK "Breeze Core removed.$\n$\nYour configuration and device tokens were kept in:$\n    %ProgramData%\breeze-core$\n$\nA paired V3 unit's token and key cannot be obtained from Midea again, so that folder is worth keeping. Delete it by hand if you are sure."
SectionEnd
