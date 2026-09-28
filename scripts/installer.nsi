; installer.nsi - Syn for Windows as one setup.exe.
;
; Built by scripts/package.ps1 from the folder it stages (or by hand):
;   makensis -DVERSION=0.1.0 -DSRC=dist\Syn-0.1.0-win-x64 -DOUT=dist\Syn-0.1.0-setup.exe scripts\installer.nsi
; makensis runs on Windows and Linux alike; the output is the same.
;
; Per user, into %LOCALAPPDATA%\Programs\Syn, with no administrator prompt:
; Syn keeps its chats and saved keys beside its own .env, so it has to live
; somewhere its user can write, which Program Files is not. The .env and the
; .agent folder are the user's: an upgrade never overwrites them, and the
; uninstaller asks before deleting them.

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !error "pass -DVERSION=<x.y.z>"
!endif
!ifndef SRC
  !error "pass -DSRC=<the staged package folder>"
!endif
!ifndef OUT
  !define OUT "Syn-${VERSION}-setup.exe"
!endif

; File looks paths up the host's way: the Windows makensis found nothing at
; "D:\...\Syn-0.1.0-win-x64/ui.exe" (CI run #49), and the Linux one wants
; "/". NSIS_WIN32_MAKENSIS is defined only by the Windows build.
!ifdef NSIS_WIN32_MAKENSIS
  !define P "${SRC}\"
!else
  !define P "${SRC}/"
!endif

!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Syn"

Name "Syn ${VERSION}"
OutFile "${OUT}"
RequestExecutionLevel user
InstallDir "$LOCALAPPDATA\Programs\Syn"
InstallDirRegKey HKCU "Software\Syn" "InstallDir"
SetCompressor /SOLID lzma
BrandingText "Syn ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Syn"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Syn setup: Excel, Word and PowerPoint, driven by a model"
VIAddVersionKey "LegalCopyright" "See LICENSE"

!define MUI_ICON "${P}syn.ico"
!define MUI_UNICON "${P}syn.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "Syn lets a language model work in the Excel, Word and PowerPoint you already have open, while you watch.$\r$\n$\r$\nIt needs Windows 10 or 11 and desktop Microsoft Office. Nothing else is installed with it.$\r$\n$\r$\nClose Syn if it is running, then continue."
!define MUI_FINISHPAGE_RUN "$INSTDIR\Syn.cmd"
!define MUI_FINISHPAGE_RUN_TEXT "Start Syn now"
!define MUI_FINISHPAGE_SHOWREADME "$INSTDIR\START HERE.txt"
!define MUI_FINISHPAGE_SHOWREADME_TEXT "Show START HERE.txt"
!define MUI_FINISHPAGE_SHOWREADME_NOTCHECKED

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "${P}LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Section "Syn" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"
  ; A running Syn holds these open; NSIS then offers Retry once it is closed.
  File "${P}ui.exe"
  File "${P}cli.exe"
  File "${P}mcpgate.exe"
  File "${P}office-host.exe"
  File "${P}Syn.cmd"
  File "${P}syn.ico"
  File "${P}LICENSE"
  File "${P}START HERE.txt"
  ; The settings are the user's once written: an upgrade keeps them.
  SetOverwrite off
  File "${P}.env"
  SetOverwrite on

  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Syn.lnk" "$INSTDIR\Syn.cmd" "" "$INSTDIR\syn.ico" 0 SW_SHOWMINIMIZED "" "Syn: Excel, Word and PowerPoint, driven by a model"
  CreateShortcut "$DESKTOP\Syn.lnk" "$INSTDIR\Syn.cmd" "" "$INSTDIR\syn.ico" 0 SW_SHOWMINIMIZED "" "Syn: Excel, Word and PowerPoint, driven by a model"

  WriteRegStr HKCU "Software\Syn" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayName" "Syn"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINST_KEY}" "Publisher" "Syn"
  WriteRegStr HKCU "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\syn.ico"
  WriteRegStr HKCU "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINST_KEY}" "NoRepair" 1
  ; Kilobytes, for Settings > Apps.
  WriteRegDWORD HKCU "${UNINST_KEY}" "EstimatedSize" 46000
SectionEnd

Section "Uninstall"
  Delete "$INSTDIR\ui.exe"
  Delete "$INSTDIR\cli.exe"
  Delete "$INSTDIR\mcpgate.exe"
  Delete "$INSTDIR\office-host.exe"
  Delete "$INSTDIR\Syn.cmd"
  Delete "$INSTDIR\syn.ico"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\START HERE.txt"
  Delete "$INSTDIR\uninstall.exe"
  Delete "$SMPROGRAMS\Syn.lnk"
  Delete "$DESKTOP\Syn.lnk"
  DeleteRegKey HKCU "${UNINST_KEY}"
  DeleteRegKey HKCU "Software\Syn"

  ; Chats, saved API keys and settings stay unless the user says otherwise:
  ; a reinstall should find them where they were. A silent uninstall keeps
  ; them.
  IfSilent keep
  MessageBox MB_YESNO|MB_DEFBUTTON2|MB_ICONQUESTION \
    "Also delete your Syn chats, saved API keys and settings?$\r$\n$\r$\n(They are in $INSTDIR. Your Office documents are never touched.)" \
    IDNO keep
  RMDir /r "$INSTDIR\.agent"
  Delete "$INSTDIR\.env"
  keep:
  ; Removed only if nothing of the user's is left in it.
  RMDir "$INSTDIR"
SectionEnd
