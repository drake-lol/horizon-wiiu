#!/bin/sh
# Start horizon-real on the Wii U and watch it: a TV screenshot and the console log every 30 s.
cd ~/wiiu/repo/wiiu/tools
W="python3 wiiuctl.py --host 192.168.87.232"
OUT=~/wiiu/hw; rm -rf $OUT; mkdir -p $OUT
if [ "$1" = "relaunch" ]; then
  timeout 20 $W reboot
  for i in $(seq 1 24); do python3 -c "import socket;socket.create_connection(('192.168.87.232',4410),2)" 2>/dev/null || break; sleep 2; done
  for i in $(seq 1 40); do sleep 5; timeout 10 $W ping >/dev/null 2>&1 && break; done
  sleep 15
fi
timeout 20 $W runsd wiiu/apps/horizon/horizon-real.rpx; echo "runsd rc $?"
start=$(date +%s)
for i in $(seq 1 ${SHOTS:-30}); do
  sleep 30
  t=$(( $(date +%s) - start ))
  timeout 20 $W info 2>&1 | tail -1
  timeout 20 $W shot --scale 2 $OUT/tv-$t.png 2>&1 | tail -1
  timeout 60 $W fetch horizon/console.txt $OUT/console.txt 2>/dev/null && echo "t=$t console $(wc -l < $OUT/console.txt) lines: $(tail -1 $OUT/console.txt | cut -c1-160)"
done
echo DONE
