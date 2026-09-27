# Kobo sleep troubleshooting

## Status — 2026-09-27

Device: Kobo Libra H2O (N873, device code 384), i.MX6 / MXCFB,
firmware 4.38.23171. App: E-Ink Chess, hosted by Cobalt's `kobod`.
FBInk migration is deferred; these findings apply to the current Cobalt setup.

The physical rear power button now requests kernel suspend. The board remains
visible with a sleeping message. After the retry fix, the owner reports that
sleep persists and waking with the physical button works once sleep settles.

**Remaining issue:** touching the screen soon after the sleeping message appears
cancels sleep. Waiting longer before touching avoids this behavior. This is
recorded for later investigation; no touch policy change was made in this pass.
The sleeping message indicates preparation as well as actual kernel suspend,
so it is not proof that the CPU has already suspended.

## Investigation and evidence

Initial symptom: the sleeping message appeared, then the app returned to its
awake state consistently after roughly 2–3 seconds.

The first persistent trace showed:

```text
power button sleep source: external=None, usb=Some(false)
kernel suspend: entering mem
kernel suspend: mem write returned after 13 ms
kernel suspend failed: Resource busy (os error 16)
power effect Resume { generation: 1, reason: Cancelled }
```

A second attempt in that session was cancelled by touch during preparation:

```text
power effect Prepare { generation: 2 }
power effect Resume { generation: 2, reason: Touch }
```

The next test, with the screen left untouched, showed a suspend write returning
in 513 ms with OS error 2. The corresponding kernel messages identified the
EPDC display controller:

```text
Freezing user space processes ... (elapsed 0.001 seconds) done.
Freezing remaining freezable tasks ... (elapsed 0.001 seconds) done.
PM: Entering mem sleep
imx_epdc_v2_fb 20f4000.epdc: waiting for VEE stable 3226747->3227439 ,please retry suspend later !!!
dpm_run_callback(): platform_pm_suspend+0x0/0x50 returns -2
PM: Device 20f4000.epdc failed to suspend: error -2
PM: Some devices failed to suspend, or early wake event detected
```

This attempt failed because the display power supply had not stabilized.
The sysfs write surfaced the driver's negative error as `No such file or
directory`; it does not establish that `/sys/power/state` was missing.
The earlier `EBUSY` attempt's exact kernel cause was not captured. Historical
kernel messages mentioned `gpio-keys`, but they cannot be attributed to that
attempt. No background process or RTC alarm was established as the cause of
these failures.

## Suspend sequence and implemented fixes

The current host:

1. Receives the physical power-button release and prepares the app for sleep.
2. Waits for the app's save acknowledgement and pending panel work.
3. Writes `1` to `/sys/power/state-extended` and allows two seconds to settle.
4. Saves the frontlight level, switches it off, syncs, then writes `mem` to
   `/sys/power/state`.
5. Resets `state-extended` to `0` after the write returns.
6. On transient errors 2 (`ENOENT`), 11 (`EAGAIN`), or 16 (`EBUSY`), returns the
   same suspend generation to Ready, re-arms `state-extended`, and schedules
   another attempt after two seconds. At most four retries follow the first
   attempt. The save barrier is retained, the app stays in its sleeping state,
   the board is not repainted, and the frontlight stays off between attempts.
7. On success, wakes the app when the blocking suspend write returns and
   restores the frontlight. On unretryable errors or exhausted retries,
   cancels sleep and restores the frontlight.

Retries run through the normal event loop, so input can cancel preparation or
an outstanding retry. Generation checks prevent a cancelled retry entering
suspend later. Other runtime errors and power/panel checks retain their normal
cancellation paths.

Relevant Cobalt commits on the fork's `chess` branch:

- `68629abc97b88501717dc1ab0306046deaba4e5a`: retry transient kernel suspend
  failures without resuming the chess screen.
- Earlier physical-button support includes the i.MX6 external-power policy:
  do not treat a fully charged unplugged battery as a reason to refuse sleep.
  MediaTek's restriction on suspend while externally powered remains.

Code locations in the Cobalt repository:

- `crates/kobod/src/device.rs`: pending suspend, retry scheduling, saved
  frontlight, error classification, and touch handling.
- `crates/kobod/src/power.rs`: `Power::retry_entry`, generation checks, and
  state transitions.
- `crates/kobod/src/blackbox.rs`: persistent trace controlled by
  `KOBO_BLACKBOX=1`.

The app's sleeping title and protocol handling are in `src/main.rs` in the
app repository, copied to `examples/eink-chess/src/main.rs` in the fork by
`scripts/prepare_cobalt.py`.

## Why early touch still cancels sleep

The event loop calls `power.wake(WakeReason::Touch)` on touch events while
Preparing or Ready, including the retry window. This returns a Resume effect
and invalidates pending entry. Once the kernel is truly suspended, that
userspace event handler is not running. This matches the owner's observation,
but the latest post-fix early-touch report has not been captured in a new trace.

Next investigation should distinguish a real touch from queued or synthetic
input, record the power state when it arrives, and agree on the intended touch
policy for physical-button sleep. If touch should not cancel that sleep,
change the host's touch policy for that sleep reason and cover both the initial
settling window and retries. Preserve deliberate power-button cancellation,
consume stale input on resume, and avoid accidentally moving a piece. Also
review page buttons, which currently use a Touch wake reason.

A separate limitation remains: a successful `mem` write returning is currently
reported as a PowerButton wake without verifying the hardware wake source.
Unexpected successful resumes need their own evidence and policy; the
implemented retry fix specifically covers failed suspend writes.

## Diagnostics and reproduction

Reproduce with USB unplugged. USB connection can change power behavior and
prevents a clean comparison with normal handheld use.

1. Enable `KOBO_BLACKBOX=1` for the `kobod` launch in
   `/mnt/onboard/.adds/cobalt/start.sh`.
2. Safely eject, unplug USB, launch Chess, press power once, and leave the
   screen untouched for at least 15 seconds.
3. Compare with touching during the first few seconds after the message.
4. Return to Kobo and reconnect USB to inspect `.kobo-blackbox.log` and
   `kobod.txt` at the root of the mounted volume.
5. If necessary, capture `dmesg` during the session, plus `/proc/uptime`,
   `/proc/driver/rtc`, `/proc/interrupts`, `/sys/power/wakeup_count`, and
   `/sys/power/state-extended`. `/sys/kernel/debug/suspend_stats` and
   `/sys/kernel/debug/wakeup_sources` were unavailable in this session.

A launcher snapshot taken before the app ran was insufficient: the planned
after-session snapshot did not appear. A temporary sampler taking a kernel
snapshot every two seconds for at most two minutes captured the actual EPDC
failure. It was removed, and the normal launcher restored before the retry
fix deployment. Diagnostic logs can include historical events; correlate the
session and uptime rather than attributing every message to the current test.
Blackbox tracing syncs every line and is normally off; enable it only for
investigation and remove temporary samplers afterwards.

Selected evidence is preserved above. Full development captures were saved
locally under `/tmp/eink-chess-sleep-investigation/` and are not durable project
artifacts or repository files.

## Verification and deployment

The retry fix passed `cargo test --locked -p kobod --features device-write`,
including regression tests for retry generation/cancellation and bounded
transient-error classification. The app's 12 tests and Cobalt renderer tests
also passed through `scripts/build_deploy_kobo.sh`.

The ARM package was built and installed over USB. The installed `kobod`
SHA-256 matched the build:

```text
734854660b0044afd4ffaea4c81c02c1d120c75563b4d9c15f3b3f9c69386ca9
```

Existing `puzzles.json` was preserved. On-device confirmation is the owner's
report that sleep now persists, with the early-touch limitation above.

## Reference

[KOReader's Kobo device implementation](https://github.com/koreader/koreader/blob/master/frontend/device/kobo/device.lua)
uses the same `state-extended → mem → state-extended` sequence and handles
unexpected wakes with a delayed guard and bounded retries. It also disables
Wi-Fi before suspend. This was a useful comparison, not proof that Wi-Fi
caused the failures observed here.
