; Hooks for the NSIS installer (bundle.windows.nsis.installerHooks in tauri.conf.json).

; Ask once whether to look after Adobe apps, and keep the answer for the app (it reads it at the
; next start). Silent and passive installs (self-updates, winget) and installs that already have
; an answer never ask; without an answer the app turns Adobe on only when Creative Cloud is installed.
!macro NSIS_HOOK_POSTINSTALL
  ${IfNot} ${Silent}
  ${AndIf} $PassiveMode <> 1
    Push $0
    ClearErrors
    ReadRegStr $0 HKCU "Software\The Updater 3D" "AdobeChoice"
    ${If} ${Errors}
      MessageBox MB_YESNO|MB_ICONQUESTION "Do you use Adobe Creative Cloud apps? The Updater 3D can keep them up to date too. You can change this later in Settings." /SD IDNO IDYES +3
      WriteRegStr HKCU "Software\The Updater 3D" "AdobeChoice" "no"
      Goto +2
      WriteRegStr HKCU "Software\The Updater 3D" "AdobeChoice" "yes"
    ${EndIf}
    Pop $0
  ${EndIf}
!macroend
