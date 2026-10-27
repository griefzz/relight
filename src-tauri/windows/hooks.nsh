; relight's installer hooks: an option, on by default, to put the relight command
; (bin\relight.cmd) on the user's PATH, and taking it off again when uninstalling. Only bin
; goes on the PATH, not the install folder, whose uninstall.exe would answer to "uninstall".
;
; The option's page comes in with RELIGHT_OPTIONS_PAGE, which windows/installer.nsi (Tauri's
; installer template, otherwise as it comes) inserts after the install folder page.

!include WinMessages.nsh
!include nsDialogs.nsh

; PowerShell reads and writes the user PATH as it's stored: with its %VARIABLES% left as
; they are, and at any length (NSIS strings stop at 1024 characters). The folder comes in
; RELIGHT_BIN, so no path needs quoting.
!define RELIGHT_PATH "$$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$d = $$env:RELIGHT_BIN; $$p = [string]$$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); $$parts = @($$p -split ';' | Where-Object { $$_ -ne '' -and $$_ -ne $$d })"

; Puts bin on the user PATH (once, last) with ADD 1, or takes it off with ADD 0.
!macro RELIGHT_SET_PATH ADD
  System::Call 'Kernel32::SetEnvironmentVariable(t "RELIGHT_BIN", t "$INSTDIR\bin")'
  !if ${ADD} == 1
    nsExec::Exec `powershell -NoProfile -NonInteractive -Command "${RELIGHT_PATH}; $$k.SetValue('Path', (($$parts + $$d) -join ';'), 'ExpandString')"`
  !else
    nsExec::Exec `powershell -NoProfile -NonInteractive -Command "${RELIGHT_PATH}; $$k.SetValue('Path', ($$parts -join ';'), 'ExpandString')"`
  !endif
  Pop $0
  ; Tells running programs (Explorer, so new terminals) the PATH changed. It waits on each
  ; window in turn, and some are slow to answer (seconds, here), so it passes over hung ones
  ; (2 = SMTO_ABORTIFHUNG) and gives each 100 ms: a window still gets the message after that,
  ; just without the installer waiting on it.
  System::Call 'user32::SendMessageTimeout(p ${HWND_BROADCAST}, i ${WM_SETTINGCHANGE}, p 0, t "Environment", i 2, i 100, *p .r0)'
!macroend

!macro RELIGHT_OPTIONS_PAGE
  ; Whether to put relight on the PATH: 1 or 0, or nothing until it's decided.
  Var RelightPath
  Var RelightPathBox

  Page custom RelightOptions RelightOptionsLeave

  ; On, unless turned off with /NOPATH, or on the page by an earlier install: an update,
  ; which shows no pages, keeps it as it was.
  Function RelightPathChoice
    ${If} $RelightPath != ""
      Return
    ${EndIf}
    ClearErrors
    ${GetOptions} $CMDLINE "/NOPATH" $0
    ${IfNot} ${Errors}
      StrCpy $RelightPath 0
      Return
    ${EndIf}
    ClearErrors
    ReadRegDWORD $RelightPath HKCU "${MANUPRODUCTKEY}" "AddToPath"
    ${If} ${Errors}
      StrCpy $RelightPath 1
    ${EndIf}
  FunctionEnd

  Function RelightOptions
    Call SkipIfPassive
    Call RelightPathChoice
    !insertmacro MUI_HEADER_TEXT "Options" "Choose how relight is set up."
    nsDialogs::Create 1018
    Pop $0
    ${If} $0 == error
      Abort
    ${EndIf}
    ${NSD_CreateCheckbox} 0 0 100% 12u "Add relight to the PATH"
    Pop $RelightPathBox
    ${If} $RelightPath == 1
      ${NSD_Check} $RelightPathBox
    ${EndIf}
    ${NSD_CreateLabel} 12u 16u -12u 24u "So that relight, and relight clip.mp4, work in any terminal opened after installing."
    Pop $0
    nsDialogs::Show
  FunctionEnd

  Function RelightOptionsLeave
    ${NSD_GetState} $RelightPathBox $0
    ${If} $0 == ${BST_CHECKED}
      StrCpy $RelightPath 1
    ${Else}
      StrCpy $RelightPath 0
    ${EndIf}
  FunctionEnd
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Off takes it away too, should an earlier install have put it there.
  Call RelightPathChoice
  ${If} $RelightPath == 1
    !insertmacro RELIGHT_SET_PATH 1
  ${Else}
    !insertmacro RELIGHT_SET_PATH 0
  ${EndIf}
  WriteRegDWORD HKCU "${MANUPRODUCTKEY}" "AddToPath" $RelightPath
!macroend

; After, not before: before comes the check for relight running, and declining to close it
; stops the uninstall, which would leave relight installed without its command.
!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro RELIGHT_SET_PATH 0
  ; An update uninstalls first: the choice stays for the install after.
  ${If} $UpdateMode <> 1
    DeleteRegValue HKCU "${MANUPRODUCTKEY}" "AddToPath"
    DeleteRegKey /ifempty HKCU "${MANUPRODUCTKEY}"
    DeleteRegKey /ifempty HKCU "${MANUKEY}"
  ${EndIf}
!macroend
