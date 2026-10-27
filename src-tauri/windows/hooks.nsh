; relight's installer hooks: put the relight command (bin\relight.cmd) on the user's PATH
; when installing, and take it off when uninstalling. Only bin goes on the PATH, not the
; install folder, whose uninstall.exe would otherwise answer to "uninstall".

!include WinMessages.nsh

; PowerShell reads and writes the user PATH as it's stored: with its %VARIABLES% left as
; they are, and at any length (NSIS strings stop at 1024 characters). The folder comes in
; RELIGHT_BIN, so no path needs quoting.
!define RELIGHT_PATH "$$k = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $$d = $$env:RELIGHT_BIN; $$p = [string]$$k.GetValue('Path', '', 'DoNotExpandEnvironmentNames'); $$parts = @($$p -split ';' | Where-Object { $$_ -ne '' -and $$_ -ne $$d })"

!macro RELIGHT_SET_PATH SCRIPT
  System::Call 'Kernel32::SetEnvironmentVariable(t "RELIGHT_BIN", t "$INSTDIR\bin")'
  nsExec::Exec `powershell -NoProfile -NonInteractive -Command "${RELIGHT_PATH}; ${SCRIPT}"`
  Pop $0
  ; Tells running programs (Explorer, so new terminals) the PATH changed.
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
!macroend

!macro NSIS_HOOK_POSTINSTALL
  ; Last on the PATH, once, however many times it's installed or updated.
  !insertmacro RELIGHT_SET_PATH "$$k.SetValue('Path', (($$parts + $$d) -join ';'), 'ExpandString')"
!macroend

; After, not before: before comes the check for relight running, and declining to close it
; stops the uninstall, which would leave relight installed without its command.
!macro NSIS_HOOK_POSTUNINSTALL
  !insertmacro RELIGHT_SET_PATH "$$k.SetValue('Path', ($$parts -join ';'), 'ExpandString')"
!macroend
