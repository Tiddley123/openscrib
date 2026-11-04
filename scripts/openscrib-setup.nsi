; The Windows installer. scripts\package-windows.ps1 runs this over the release
; folder, so it packs the signed exes rather than rebuilding them:
;   makensis /DVERSION=0.1.3 /DSOURCE=<folder> /DOUTFILE=<setup.exe> openscrib-setup.nsi
;
; It installs for one user with no admin rights, or for everyone in Program
; Files. Setup itself always runs as the user: for an all-users install it
; starts a silent elevated copy of itself just to write the files, so what
; follows (replacing old copies, opening the app) still runs as that person
; and not as an administrator, who may be a different account.
;
;   setup.exe /S                     per-user, silently
;   setup.exe /S /ALLUSERS           all users, from an elevated prompt
;   setup.exe /S /D=C:\Some\Folder   /D goes last and takes no quotes
;
; Installing again over an older install just replaces the program, in the
; folder and scope it already has. Recordings are in Documents\OpenSCRIB and
; models in %LOCALAPPDATA%\OpenSCRIB, per user either way, and this script
; never deletes from them, on update or on uninstall.

Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "nsDialogs.nsh"

!ifndef VERSION | SOURCE | OUTFILE
  !error "Pass /DVERSION, /DSOURCE and /DOUTFILE"
!endif

!define PRODUCT "OpenSCRIB"
!define PUBLISHER "Rovamp Services Pty Ltd"
!define APP_EXE "OpenSCRIB.exe"
!define UNINSTALLER "Uninstall OpenSCRIB.exe"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${PRODUCT}"
!define PROGID "OpenSCRIB.Project"
!define USER_DIR "$LOCALAPPDATA\Programs\${PRODUCT}"
!define ALL_USERS_DIR "$PROGRAMFILES64\${PRODUCT}"

Name "${PRODUCT}"
OutFile "${OUTFILE}"
InstallDir "${USER_DIR}"
RequestExecutionLevel user
SetCompressor /SOLID lzma
ShowInstDetails nevershow
ShowUninstDetails nevershow
BrandingText " "

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "${PRODUCT}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "${PRODUCT} setup"
VIAddVersionKey "CompanyName" "${PUBLISHER}"
VIAddVersionKey "LegalCopyright" "Copyright ${U+00A9} 2026 ${PUBLISHER}"

Var AllUsers   ; 1 when this install is for everyone
Var IsAdmin    ; 1 when this process is elevated
Var Existing   ; the folder of the install this one updates, if any
Var FolderText ; heading on the folder page
Var RadioUser
Var UserDir    ; held because $LOCALAPPDATA changes once the context is "all"

!define MUI_ICON "..\app\src-tauri\icons\icon.ico"
!define MUI_UNICON "..\app\src-tauri\icons\icon.ico"
!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TEXT "This installs ${PRODUCT} ${VERSION}. You can choose who it's for and which folder it goes in.$\r$\n$\r$\nIf you have an older copy, setup can replace it. Your recordings are kept."
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION OpenApp
!define MUI_FINISHPAGE_RUN_TEXT "Open ${PRODUCT}"

!insertmacro MUI_PAGE_WELCOME
Page custom ScopePage ScopePageLeave
!define MUI_PAGE_CUSTOMFUNCTION_PRE FolderPagePre
!define MUI_PAGE_CUSTOMFUNCTION_SHOW FolderPageShow
!define MUI_DIRECTORYPAGE_TEXT_TOP "$FolderText"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

!macro DetectAdmin
  StrCpy $IsAdmin 0
  UserInfo::GetAccountType
  Pop $0
  ${If} $0 == "Admin"
    StrCpy $IsAdmin 1
  ${EndIf}
!macroend

Function .onInit
  SetRegView 64
  !insertmacro DetectAdmin
  StrCpy $UserDir "${USER_DIR}"
  StrCpy $AllUsers 0
  ${GetParameters} $0
  ClearErrors
  ${GetOptions} $0 "/ALLUSERS" $1
  ${IfNot} ${Errors}
    StrCpy $AllUsers 1
  ${EndIf}

  ; An update stays where it is. Moving means uninstalling first, which keeps
  ; two copies from ever fighting over the file association.
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} ${FileExists} "$0\${APP_EXE}"
    StrCpy $Existing $0
    StrCpy $AllUsers 1
  ${Else}
    ReadRegStr $0 HKCU "${UNINSTALL_KEY}" "InstallLocation"
    ${If} ${FileExists} "$0\${APP_EXE}"
      StrCpy $Existing $0
    ${EndIf}
  ${EndIf}

  ; $INSTDIR is still the default here unless /D gave another.
  ${If} $INSTDIR == $UserDir
    ${If} $Existing != ""
      StrCpy $INSTDIR $Existing
    ${ElseIf} $AllUsers == 1
      StrCpy $INSTDIR "${ALL_USERS_DIR}"
    ${EndIf}
  ${EndIf}

  ${If} $AllUsers == 1
    SetShellVarContext all
    ${If} ${Silent}
    ${AndIf} $IsAdmin != 1
      SetErrorLevel 5
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

Function ScopePage
  ${If} $Existing != ""
    Abort
  ${EndIf}
  !insertmacro MUI_HEADER_TEXT "Who is this for?" "Choose who can use ${PRODUCT} on this computer."
  nsDialogs::Create 1018
  Pop $0

  ${NSD_CreateRadioButton} 0 0 100% 12u "Just me"
  Pop $RadioUser
  ${NSD_CreateLabel} 12u 13u 95% 18u "No administrator rights needed. Installs to:$\r$\n$UserDir"
  Pop $0
  ${NSD_CreateRadioButton} 0 44u 100% 12u "Everyone on this computer"
  Pop $1
  ${NSD_CreateLabel} 12u 57u 95% 18u "Windows will ask for administrator approval. Installs to:$\r$\n${ALL_USERS_DIR}"
  Pop $0
  ${NSD_CreateLabel} 0 88u 100% 18u "Either way, each person's recordings stay in their own Documents\OpenSCRIB folder."
  Pop $0

  ${If} $AllUsers == 1
    ${NSD_Check} $1
  ${Else}
    ${NSD_Check} $RadioUser
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function ScopePageLeave
  ${NSD_GetState} $RadioUser $0
  ${If} $0 == ${BST_CHECKED}
    StrCpy $AllUsers 0
    SetShellVarContext current
  ${Else}
    StrCpy $AllUsers 1
    SetShellVarContext all
  ${EndIf}
  ; Leave a folder they typed in alone when they come Back and change this.
  ${If} $INSTDIR == $UserDir
  ${OrIf} $INSTDIR == "${ALL_USERS_DIR}"
    ${If} $AllUsers == 1
      StrCpy $INSTDIR "${ALL_USERS_DIR}"
    ${Else}
      StrCpy $INSTDIR $UserDir
    ${EndIf}
  ${EndIf}
FunctionEnd

Function FolderPagePre
  ${If} $Existing != ""
    StrCpy $FolderText "${PRODUCT} is already installed here and will be updated in this folder. To move it, uninstall it first: your recordings are kept."
  ${ElseIf} $AllUsers == 1
    StrCpy $FolderText "${PRODUCT} will be installed for everyone in this folder. Windows will ask for administrator approval when you choose Install."
  ${Else}
    StrCpy $FolderText "${PRODUCT} will be installed for your account in this folder. To use another, choose Browse."
  ${EndIf}
FunctionEnd

; The folder box and Browse are items 1019 and 1001 of the inner dialog.
Function FolderPageShow
  ${If} $Existing != ""
    FindWindow $0 "#32770" "" $HWNDPARENT
    GetDlgItem $1 $0 1019
    EnableWindow $1 0
    GetDlgItem $1 $0 1001
    EnableWindow $1 0
  ${EndIf}
FunctionEnd

; Through Explorer, so it opens as the user even from an elevated setup.
Function OpenApp
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\${APP_EXE}"'
FunctionEnd

; A running exe can't be opened for writing. Nor can one a virus scanner is
; still reading, so give that a moment before blaming the app.
!macro WaitForAppToClose un
Function ${un}WaitForAppToClose
  retry:
    StrCpy $1 0
  check:
    IfFileExists "$INSTDIR\${APP_EXE}" 0 done
    ClearErrors
    FileOpen $0 "$INSTDIR\${APP_EXE}" a
    IfErrors 0 closed
    IntOp $1 $1 + 1
    IntCmp $1 10 running
    Sleep 500
    Goto check
  closed:
    FileClose $0
    Goto done
  running:
    ; No /SD: an elevated copy is silent, but someone is watching its parent.
    ${If} ${Silent}
    ${AndIf} $AllUsers != 1
      SetErrorLevel 2
      Quit
    ${EndIf}
    MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "${PRODUCT} is open. Close it, then choose Retry." IDRETRY retry
    SetErrorLevel 2
    Quit
  done:
FunctionEnd
!macroend
!insertmacro WaitForAppToClose ""
!insertmacro WaitForAppToClose "un."

; Offers to replace each portable copy in turn, asking about every one: the
; openscrib-cli "old-copies" commands do the work and word the messages. A copy
; that is replaced drops off the list, so only a skipped one moves the index on.
Function ReplaceOldCopies
  StrCpy $R0 0
  next:
    nsExec::ExecToStack '"$INSTDIR\openscrib-cli.exe" old-copies describe $R0'
    Pop $R1
    Pop $R2
    StrCmp $R1 "0" 0 finished
    MessageBox MB_YESNO|MB_ICONQUESTION "$R2" IDYES replace
    IntOp $R0 $R0 + 1
    Goto next
  replace:
    DetailPrint "Replacing the older copy"
    nsExec::ExecToStack '"$INSTDIR\openscrib-cli.exe" old-copies replace $R0'
    Pop $R1
    Pop $R2
    StrCmp $R1 "0" replaced
    MessageBox MB_OK|MB_ICONEXCLAMATION "$R2"
    IntOp $R0 $R0 + 1
    Goto next
  replaced:
    MessageBox MB_OK|MB_ICONINFORMATION "$R2"
    Goto next
  finished:
FunctionEnd

; Inserted into the section, not called, so setup can count the files' size.
!macro InstallFiles
  Call WaitForAppToClose
  SetOutPath "$INSTDIR"
  File "${SOURCE}\${APP_EXE}"
  File "${SOURCE}\openscrib-cli.exe"
  File "${SOURCE}\openscrib-vision.exe"
  File "${SOURCE}\msvcp140.dll"
  File "${SOURCE}\vcruntime140.dll"
  File "${SOURCE}\vcruntime140_1.dll"
  File "${SOURCE}\vcomp140.dll"
  File "${SOURCE}\LICENSE-MIT.txt"
  ; Tells the app to keep its data outside this folder (see paths.rs).
  FileOpen $0 "$INSTDIR\openscrib-installed" w
  FileClose $0

  WriteUninstaller "$INSTDIR\${UNINSTALLER}"
  CreateShortcut "$SMPROGRAMS\${PRODUCT}.lnk" "$INSTDIR\${APP_EXE}"

  ; SHCTX is HKLM for everyone and HKCU for one user (SetShellVarContext).
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "DisplayName" "${PRODUCT}"
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE}"
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\${UNINSTALLER}"'
  WriteRegStr SHCTX "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\${UNINSTALLER}" /S'
  WriteRegDWORD SHCTX "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD SHCTX "${UNINSTALL_KEY}" "NoRepair" 1

  WriteRegStr SHCTX "Software\Classes\.scrib" "" "${PROGID}"
  WriteRegStr SHCTX "Software\Classes\${PROGID}" "" "OpenSCRIB recording"
  WriteRegStr SHCTX "Software\Classes\${PROGID}\DefaultIcon" "" "$INSTDIR\${APP_EXE},0"
  WriteRegStr SHCTX "Software\Classes\${PROGID}\shell\open\command" "" '"$INSTDIR\${APP_EXE}" "%1"'
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
!macroend

; The elevated copy reports only that it ran, so the registry says whether it
; wrote this version. A cancelled approval prompt sets the error flag.
Function InstallElevated
  DetailPrint "Waiting for administrator approval"
  ClearErrors
  ExecShellWait "runas" "$EXEPATH" "/S /ALLUSERS /D=$INSTDIR"
  ${If} ${Errors}
    MessageBox MB_OK|MB_ICONEXCLAMATION "Windows didn't give setup administrator approval, so nothing was installed. Run setup again, or choose Just me."
    Abort "Cancelled"
  ${EndIf}
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "DisplayVersion"
  ReadRegStr $1 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 != "${VERSION}"
  ${OrIf} $1 != $INSTDIR
    MessageBox MB_OK|MB_ICONEXCLAMATION "${PRODUCT} couldn't be installed in $INSTDIR."
    Abort "Failed"
  ${EndIf}
FunctionEnd

Section "Install"
  ${If} $AllUsers == 1
  ${AndIf} $IsAdmin != 1
    Call InstallElevated
  ${Else}
    !insertmacro InstallFiles
  ${EndIf}

  ; Only the first install: the question is about moving to this one, and
  ; someone who kept a portable copy shouldn't get it again with every update.
  ${If} $Existing == ""
  ${AndIfNot} ${Silent}
    Call ReplaceOldCopies
  ${EndIf}
SectionEnd

Function un.onInit
  SetRegView 64
  StrCpy $AllUsers 0
  ReadRegStr $0 HKLM "${UNINSTALL_KEY}" "InstallLocation"
  ${If} $0 == $INSTDIR
    StrCpy $AllUsers 1
    SetShellVarContext all
    !insertmacro DetectAdmin
    ${If} $IsAdmin != 1
      ${If} ${Silent}
        ExecShell "runas" "$INSTDIR\${UNINSTALLER}" "/S"
      ${Else}
        ExecShell "runas" "$INSTDIR\${UNINSTALLER}"
      ${EndIf}
      Quit
    ${EndIf}
  ${EndIf}
FunctionEnd

; By name, so nothing that isn't ours can go with it.
Section "Uninstall"
  Call un.WaitForAppToClose

  Delete "$INSTDIR\${APP_EXE}"
  Delete "$INSTDIR\openscrib-cli.exe"
  Delete "$INSTDIR\openscrib-vision.exe"
  Delete "$INSTDIR\msvcp140.dll"
  Delete "$INSTDIR\vcruntime140.dll"
  Delete "$INSTDIR\vcruntime140_1.dll"
  Delete "$INSTDIR\vcomp140.dll"
  Delete "$INSTDIR\LICENSE-MIT.txt"
  Delete "$INSTDIR\openscrib-installed"
  Delete "$INSTDIR\${UNINSTALLER}"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${PRODUCT}.lnk"

  ReadRegStr $0 SHCTX "Software\Classes\.scrib" ""
  ${If} $0 == "${PROGID}"
    DeleteRegKey SHCTX "Software\Classes\.scrib"
  ${EndIf}
  DeleteRegKey SHCTX "Software\Classes\${PROGID}"
  DeleteRegKey SHCTX "${UNINSTALL_KEY}"
  System::Call 'shell32::SHChangeNotify(i 0x08000000, i 0, p 0, p 0)'
SectionEnd
