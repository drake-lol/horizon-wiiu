#!/bin/sh
# usage: runcemu.sh file.rpx  -> runs under Xvfb, writes /out/shot.png and /out/cemu.log
export HOME=/out/home XDG_CONFIG_HOME=/out/home/.config XDG_DATA_HOME=/out/home/.local/share LIBGL_ALWAYS_SOFTWARE=1 GALLIUM_DRIVER=llvmpipe
mkdir -p $HOME
Xvfb :99 -screen 0 1280x720x24 >/dev/null 2>&1 &
export DISPLAY=:99
sleep 2
/cemu/squashfs-root/usr/bin/Cemu -g "$1" > /out/cemu.log 2>&1 &
sleep 45
ffmpeg -loglevel error -y -f x11grab -video_size 1280x720 -i :99 -frames:v 1 /out/shot.png
pkill Cemu; true
