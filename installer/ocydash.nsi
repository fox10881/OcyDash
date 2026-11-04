; OcyDash — NSIS installer script
; Build: makensis /V4 installer\ocydash.nsi
; Produces dist\OcyDash-Setup-<ver>.exe

Unicode true
!define APP_NAME   "OcyDash"
!define APP_EXE    "ocydash.exe"
!define APP_VER    "0.1.0"
!define APP_DIR    "$PROGRAMFILES64\OcyDash"
!define APP_UNINST "Software\Microsoft\Windows\CurrentVersion\Uninstall\OcyDash"
!define APP_RUNKEY "Software\Microsoft\Windows\CurrentVersion\Run"

!include "MUI2.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

Name "${APP_NAME}"
OutFile "..\dist\OcyDash-Setup-${APP_VER}.exe"
InstallDir "${APP_DIR}"
InstallDirRegKey HKLM "${APP_UNINST}" "InstallLocation"
RequestExecutionLevel admin
SetCompressor /SOLID lzma

!define MUI_ICON   "..\assets\ocydash.ico"
!define MUI_UNICON "..\assets\ocydash.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP_EXE}"
!define MUI_FINISHPAGE_RUN_TEXT "Launch ${APP_NAME} (tray service)"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES

!insertmacro MUI_LANGUAGE "English"

Section "Install"
    SetShellVarContext all
    SetOutPath "$INSTDIR"

    ; stop a running service/editor so files aren't locked
    nsExec::ExecToStack 'taskkill /F /IM ${APP_EXE}'
    Pop $0
    Pop $1

    File "..\target\release\${APP_EXE}"
    File /nonfatal "..\vendor\ffmpeg\ffmpeg.exe"   ; bundled if present
    File /nonfatal "..\LICENSE"
    File /nonfatal "..\README.md"
    File "..\assets\ocydash.ico"

    ; sensor feed helper + ALL of its dependencies — recursive copy so
    ; any dll/sys/subfolder added later is packaged automatically
    SetOutPath "$INSTDIR\SystemInfos"
    File /r /x README.txt "..\vendor\SystemInfos\*.*"
    SetOutPath "$INSTDIR"

    WriteUninstaller "$INSTDIR\uninstall.exe"

    ; shortcuts — no args = headless tray service
    CreateDirectory "$SMPROGRAMS\OcyDash"
    CreateShortcut  "$SMPROGRAMS\OcyDash\OcyDash.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
    CreateShortcut  "$SMPROGRAMS\OcyDash\OcyDash Editor.lnk" "$INSTDIR\${APP_EXE}" "--editor" "$INSTDIR\${APP_EXE}" 0
    CreateShortcut  "$SMPROGRAMS\OcyDash\Uninstall.lnk" "$INSTDIR\uninstall.exe"
    CreateShortcut  "$DESKTOP\OcyDash.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0

    ; Add/Remove Programs entry
    WriteRegStr HKLM "${APP_UNINST}" "DisplayName" "${APP_NAME}"
    WriteRegStr HKLM "${APP_UNINST}" "DisplayVersion" "${APP_VER}"
    WriteRegStr HKLM "${APP_UNINST}" "Publisher" "OcyDash"
    WriteRegStr HKLM "${APP_UNINST}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
    WriteRegStr HKLM "${APP_UNINST}" "InstallLocation" "$INSTDIR"
    WriteRegStr HKLM "${APP_UNINST}" "UninstallString" '"$INSTDIR\uninstall.exe"'
    WriteRegDWORD HKLM "${APP_UNINST}" "NoModify" 1
    WriteRegDWORD HKLM "${APP_UNINST}" "NoRepair" 1
    ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
    IntFmt $0 "0x%08X" $0
    WriteRegDWORD HKLM "${APP_UNINST}" "EstimatedSize" $0
SectionEnd

Section "Uninstall"
    SetShellVarContext all
    nsExec::ExecToStack 'taskkill /F /IM ${APP_EXE}'
    Pop $0
    Pop $1

    ; remove autostart — Task Scheduler task (elevated apps can't use
    ; the Run key) plus the legacy HKCU value from earlier builds
    nsExec::ExecToStack 'schtasks /delete /tn "OcyDash" /f'
    Pop $0
    Pop $1
    DeleteRegValue HKCU "${APP_RUNKEY}" "OcyDash"

    Delete "$DESKTOP\OcyDash.lnk"
    RMDir /r "$SMPROGRAMS\OcyDash"

    Delete "$INSTDIR\${APP_EXE}"
    Delete "$INSTDIR\ffmpeg.exe"
    Delete "$INSTDIR\ocydash.ico"
    Delete "$INSTDIR\LICENSE"
    Delete "$INSTDIR\README.md"
    Delete "$INSTDIR\uninstall.exe"
    RMDir /r "$INSTDIR\SystemInfos"
    RMDir "$INSTDIR"

    DeleteRegKey HKLM "${APP_UNINST}"
    ; user themes (%APPDATA%\ocydash) intentionally kept
SectionEnd
