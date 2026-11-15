# Horizon test bridge (Aroma plugin)

Automated testing on a real Wii U running [Aroma](https://aroma.foryour.cafe/): a computer on the same network
sees what the TV and GamePad show, presses GamePad buttons, moves the sticks, touches the screen, sends and starts
homebrew, and returns to the menu, reboots or powers off the console.

- `src/main.cpp`: the plugin. TCP port **4410**, one text command per line (the list is at the top of the file).
  Screenshots come from the frame the running application hands to the screen; inputs are merged into what the
  GamePad reports, so a test drives any application, the Wii U Menu included.
- `../tools/wiiuctl.py`: the client, Python 3 standard library only. Also talks to Aroma's own plugins:
  wiiload (TCP 4299, send and start an `.rpx`/`.wuhb`), ftpiiu (TCP 21, copy files), and the logging module's
  UDP broadcast (4405).

## Install

1. Copy `dist/HorizonTestBridge.wps` to the SD card's `wiiu/environments/aroma/plugins/`
   (or, with Aroma's FTP plugin on: `wiiuctl.py --host <console ip> install dist/HorizonTestBridge.wps`).
2. Restart the console. The plugin is listed in Aroma's plugin menu (L + Down + Select).
3. `export WIIU=<console ip>` and `wiiuctl.py ping`.

For `run`, enable Aroma's Wiiload plugin; for `logs`, its logging module; for `install`, its FTP plugin.

## Use

```sh
wiiuctl.py shot tv.png                 # what the TV shows (--drc: the GamePad; --scale 2: half size)
wiiuctl.py press A                     # A B X Y L R ZL ZR PLUS MINUS HOME UP DOWN LEFT RIGHT STICK_L STICK_R
wiiuctl.py press ZR+B 500              # together, for 500 ms
wiiuctl.py hold ZR && wiiuctl.py release ZR
wiiuctl.py stick L 0 1 1000            # left stick up for a second (no ms: until `clear`)
wiiuctl.py touch 427 240               # GamePad screen pixels (854 x 480)
wiiuctl.py run ../horizon-real/target/powerpc-nintendo-wiiu/release/horizon-real.rpx
wiiuctl.py logs                        # console output while a program runs
wiiuctl.py watch shots/                # a screenshot every 5 s
wiiuctl.py menu | reboot | shutdown | launch <title id>   # reboot relaunches the OS and Aroma, no power cycle
wiiuctl.py script smoke.txt            # a test: commands, `wait <ms>`, `shot <file>`, `run <rpx>`, `expect-ping`
```

A test script, for example (`smoke.txt`):

```
run horizon-real.rpx
wait 60000
shot title.png
press A
wait 2000
shot after-a.png
menu
expect-ping 60
```

From Python: `from wiiuctl import WiiU; w = WiiU("192.168.1.50"); w.press("A"); w.shot("x.png")`.

## Build

```sh
docker build -t wups-build -f ../tools/Dockerfile.wups ../tools
docker run --rm -v "$PWD":/project wups-build make      # HorizonTestBridge.wps
```

## Notes

- HOME is injected like any button, but the system reads the GamePad's HOME button on its own, so the HOME Menu
  may not open from an injected press. `menu` returns to the Wii U Menu instead.
- Screenshots ask no work of the GPU (a GPU copy hung real consoles). The plugin learns where the application's
  scan buffers are from `GX2SetTVBuffer`/`GX2SetDRCBuffer`, marks a few pixels of each slot, and after the
  application's own `GX2CopyColorBufferToScanBuffer` reads the slot whose marks were overwritten, untiling it
  (2D_TILED_THIN1, 32-bit) on the CPU. `GX2GetLastFrame` is not used: it returns a frame saved when the application
  started. With no frame presented for 5 s (a paused or hung application) `shot` reports so.
- Diagnostics: `SCANINFO` (where the scan buffers are), `SHOTRAW TV|DRC` (a slot's bytes untouched), and `INFO`
  counts `VPADRead` calls.
- Tested on a real console: screenshots of both screens, button presses, `reboot`. Not yet: touch, `shutdown`,
  `launch`, and `run` (Aroma's Wiiload stopped reading halfway through a 100 KB upload; under investigation).
- The server restarts with each application; `wiiuctl.py` reconnects by itself.
- Anyone on the network can reach port 4410 while the plugin is loaded: disable it in Aroma's plugin menu when
  not testing.
