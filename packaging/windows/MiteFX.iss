; MiteFX for Windows: an Inno Setup installer (.github/workflows/release.yml builds it).
; ISCC /DSrc=<folder with VST3\ CLAP\ LICENSE.txt> /DVersion=0.1.0 /O<out> MiteFX.iss

#ifndef Version
  #define Version "0.0.0"
#endif

[Setup]
AppId={{6F1D2C55-9B0E-4E57-9C1A-4D1E5E3A7F10}
AppName=MiteFX
AppVersion={#Version}
AppVerName=MiteFX {#Version}
AppPublisher=MiteFX
AppPublisherURL=https://miteffects.github.io/MiteFX/
AppSupportURL=https://github.com/miteffects/MiteFX/issues
DefaultDirName={commonpf64}\MiteFX
DisableDirPage=yes
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
PrivilegesRequired=admin
LicenseFile={#Src}\LICENSE.txt
SetupIconFile=mite.ico
UninstallDisplayIcon={app}\mite.ico
UninstallDisplayName=MiteFX (audio plugins)
OutputBaseFilename=MiteFX-Windows-Setup
; One 512 MB window over everything: the 19 plugins share most of their code, so it is stored
; about once (decompressing needs that much memory, compressing ~6 GB in islzma64).
Compression=lzma2/ultra64
LZMADictionarySize=524288
LZMANumFastBytes=273
LZMAUseSeparateProcess=yes
SolidCompression=yes
WizardStyle=modern

[Types]
Name: "full"; Description: "VST3 and CLAP"
Name: "custom"; Description: "Choose formats"; Flags: iscustom

[Components]
Name: "vst3"; Description: "VST3 plugins (C:\Program Files\Common Files\VST3)"; Types: full custom
Name: "clap"; Description: "CLAP plugins (C:\Program Files\Common Files\CLAP)"; Types: full custom

[Files]
Source: "{#Src}\VST3\*"; DestDir: "{commoncf64}\VST3"; Components: vst3; Flags: recursesubdirs createallsubdirs ignoreversion
; Each CLAP is the same binary as its VST3: clap-files.iss (written by the release workflow)
; installs the VST3's file under the .clap name, so the installer holds it once.
#include "clap-files.iss"
Source: "{#Src}\LICENSE.txt"; DestDir: "{app}"; Flags: ignoreversion
Source: "mite.ico"; DestDir: "{app}"; Flags: ignoreversion

[Messages]
FinishedLabel=MiteFX is installed. Rescan plugins in your DAW to find the Mite plugins.
