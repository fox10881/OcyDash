Bundled sensor helper (installed beside ocydash.exe as SystemInfos\).

SystemInfos.exe + HWiNFO64.dll are copied here from the official package:
  C:\Program Files\Ocypus Display\resources\main\SDK\VC#\SystemInfos\
    vs2008\bin\x64\Release\

These binaries are NOT committed to git (see .gitignore) — copy them into
this folder before running makensis so the installer is self-contained.
The kernel driver they load is registered by the official installer;
if it is absent the helper starts but produces no feed.
