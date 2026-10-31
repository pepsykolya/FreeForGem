# Idle mode for Windows (design, 2026-10-31)

Goal: people who play or work on a Windows PC mine only while it is idle. As soon as the user comes back,
mining stops (or drops to a light load) within a second, and the PC behaves as if no miner were running.

## User-facing

`start.bat`:
```
set "IDLE=1"            :: mine only while the PC is idle
set "IDLE_MINUTES=5"    :: no keyboard/mouse/gamepad for 5 min = idle
set "ACTIVE=pause"      :: when the user is active: pause | light (30 % load) | light:50
set "ALWAYS_PAUSE=cs2.exe,blender.exe"  :: optional: these programs pause mining even with no input
```
Flags: `--idle`, `--idle-minutes N`, `--active pause|light[:PCT]`, `--idle-pause-for exe,...`.
Log: `idle: user active (input), mining paused` / `idle: idle 5 min, mining resumed` and the time mined/paused in the summary.
HiveOS / Linux: the flags are ignored (Windows only).

## When is the PC "busy" (any one is enough)

| Signal | API | Why |
|---|---|---|
| keyboard / mouse input in the last N min | `GetLastInputInfo` | the main signal |
| gamepad | `XInputGetState` (packet number changes) | controller gaming gives no keyboard input |
| fullscreen game / presentation / "do not disturb" | `SHQueryUserNotificationState` (`QUNS_RUNNING_D3D_FULL_SCREEN`, `QUNS_BUSY`, `QUNS_PRESENTATION_MODE`) | a game the user just watches (cutscene) |
| video playing, a call in progress | `PowerRequest` display/system requests of other processes (`CallNtPowerInformation`/the same data as `powercfg /requests`) | a movie or a Zoom call has no input but must not stutter |
| another program uses the GPU | NVML `nvmlDeviceGetProcessUtilization` / running compute+graphics processes other than ours | a render, local AI, a game launched by a script |
| a program from `ALWAYS_PAUSE` runs | process list | the user's explicit wish |
| the screen is locked | `WTSRegisterSessionNotification` (lock/unlock) | locked = idle even if the timer has not run out (option, on by default) |

Hysteresis: pause at once (poll every 250 ms), resume only after `IDLE_MINUTES` with no signal. After a pause caused by a game
or a video, wait the same time again once it ends.

## What "pause" must do (the hard part)

1. **GPU work stops**: the per-GPU pause handshake already exists (`worker.rs`, `gs.pause` / `gs.paused`, used by auto-uv checks):
   in-flight attempt finishes (<10 ms), the thread waits. Same for NOID (CPU check pool too) and CMFD (stop the engine).
2. **Clocks and undervolt back to the owner's values**: a locked core clock (`--lock-cclock`) and the auto-uv offset would cap a game's
   clocks. On pause: `nvmlDeviceResetGpuLockedClocks` / memory reset + the owner's offset (`uv.rs` `restore()` already does it);
   on resume: lock + profile again (verified profiles: no new check).
3. **VRAM**: the miner keeps its buffers (~1–2 GB per GPU) — a game on an 8 GB card can run short. Phase 1 keeps them (fast resume);
   phase 2: after 60 s of pause free the GPU buffers / context and rebuild them on resume (~2–5 s).
4. **auto-uv**: tunings and re-checks run only while idle; one interrupted by the user is "interrupted", never a crash
   (no crash marker, `forget_trying`), and it resumes later.
5. **Pool**: the stratum connection stays (no shares meanwhile); a pool that drops an idle worker is reconnected on resume.
6. **Dev fee**: 1 % of the MINING time — the 100-minute cycle clock runs only while mining, a slot due during a pause is taken
   after the resume (needs a change in `devfee.rs`, ours: LICENSE clause 3 binds third parties, the accounting stays 1 %).

## "light" mode instead of a full pause

Duty cycle in the feeding loop: mine X ms, sleep Y ms (30 %: ~30 ms of 100). Needs no admin rights, works on NVIDIA and AMD; the clock
lock is lifted the same way as for a pause. Good for office work and video; for games the default stays `pause`
(any GPU load costs FPS and frame-time spikes).

## Session 0 caveat

`GetLastInputInfo` sees only the interactive session. A miner started by a scheduled task "run whether the user is logged on or
not" runs in session 0 and would always look idle. So with `IDLE=1`: run in the user's session (task "at logon", hidden window —
`install-autostart.bat` creates it); in session 0 the miner refuses idle mode with a clear message.

## Phases

1. `--idle` with input + fullscreen + locked screen, `pause` only, clocks/uv restore, auto-uv aware, fee on mining time. Test: an RTX 3080 Ti Windows PC — game, video, lock.
2. Video/calls (power requests), gamepad, other GPU processes, `ALWAYS_PAUSE`, `light` mode.
3. Free VRAM on a long pause; tray icon (mining / paused, "pause for 1 h").
