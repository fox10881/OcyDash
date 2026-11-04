Drop a Windows x64 ffmpeg.exe here before running makensis and the installer
will bundle it (video + large-GIF backgrounds).

Any recent static build works, e.g. https://www.gyan.dev/ffmpeg/builds/
(ffmpeg-release-essentials.zip -> bin/ffmpeg.exe).

At runtime OcyDash looks for ffmpeg.exe next to ocydash.exe first, then falls
back to the vendor app's copy if installed.
