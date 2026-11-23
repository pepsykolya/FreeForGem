# Idle mode for Windows

People who play or work on a Windows PC can let FreeForGem mine only while the PC is idle: as soon as someone uses the PC,
mining pauses (or drops to a light load) within a second and the card's own clocks and offsets come back.

```
set "IDLE=1"            :: mine only while the PC is idle
set "IDLE_MINUTES=5"    :: no keyboard/mouse/gamepad for 5 min = idle
set "ACTIVE=pause"      :: when the user is active: pause | light (30 % load) | light:50
set "ALWAYS_PAUSE=cs2.exe,blender.exe"  :: optional: these programs pause mining even with no input
```

Windows only (ignored on Linux / HiveOS). The full description — what counts as "in use", the `pause` / `light` modes, the log
lines, the dev fee on mining time and the session requirement — is in
[MANUAL.md](../MANUAL.md#18-windows-mine-only-while-the-pc-is-idle-new-in-175), section 18.
