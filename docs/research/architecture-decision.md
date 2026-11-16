# Architecture Decision Record: On-Device UI/Control-Surface Process Model

Status: **REVISED, 2026-11-14 (same day, post re-verification).** The original recommendation below (§§0-5, unchanged, kept for the record) was **drop React/Angular, build native Qt/QML**. New on-device measurement overturns the load-bearing assumption behind that call. **Read the REVISION section immediately below first, it is the current answer.** Everything from "## 0. Restating the problem" onward is the superseded first pass, preserved for audit trail, not current guidance.

Research date basis: 2026-11-14.

---

## REVISION (2026-11-14): Chromium-kiosk is production-validated on this exact hardware; recommendation flips back to React/Angular

**What changed:** `hardware-verification.md`'s "Live process list + real RAM measurement" section (added after this document's first pass) ran `ps aux` + `free -m` against the live, stock, fully-booted unit. Two things in the original §1/§2/§4 analysis turn out to be wrong:

1. **"Stock firmware has no Chromium, pure-QML" was a false negative.** It does run Chromium: `chromium-browser/chrome --kiosk --no-sandbox --in-process-gpu --remote-debugging-port=2222 --app=file:///usr/share/qt-superbird-app/webapp/index.html`, rendering a local HTML/JS web app. `/etc/qt-superbird-app` is the native launcher/supervisor's config, not the whole UI stack. Spotify's own production engineers already shipped a Chromium-kiosk-renders-local-web-app architecture on this exact SoC/RAM/firmware family, this is architecture (b) from §1, not (c), and not native Qt/QML.
2. **The approximately 150-250MB "Chromium tax" estimate was too pessimistic, and by a lot.** Real `free -m` with the *entire* stock stack live simultaneously, full Chromium process tree (browser + zygote + renderer w/ 2 raster threads + audio utility process) **plus** `bluetoothd`, `earlyoom`, `swupdate`, the backlight daemon, and the native launcher, measures **152MB used, 323MB available** (`MemAvailable` 330448kB) on a 488MB-total unit. That is the *whole system*, not just the browser, costing less than the low end of what §1 estimated for the browser component alone. The original §2 rule of thumb, "need to see 350MB+ free after boot before (a)/(b) stop being a real risk," is reframed by this number rather than cleanly cleared by it: 323MB is slightly under that bar, but it's measured in **steady state with the full Chromium tax already paid**, not "free before the tax is applied" as §2 implicitly meant. That's a strictly more informative and more favorable number than what §5 asked for, it's not a projection, it's the post-tax headroom, observed.

**Answering the four questions directly:**

**(1) Does this change the RAM-risk assessment?** Yes, substantially. The dominant risk flagged throughout §1 to §5, "a Chromium-lineage engine could plausibly eat 150-250MB, an unacceptable fraction of 512MB, and we have no measurement," is resolved and was too pessimistic: the *entire* reference stack, Chromium included, costs 152MB, with 323MB free afterward. The error wasn't in the Chromium-engine-is-heavy direction (it should be, roughly), it was in assuming the rest of the budget (kernel, BusyBox, BT, OOM/update/backlight daemons) would be cheap enough that the 150-250MB browser estimate would still dominate; in fact the composite is far cheaper than the single-component worst case implied. The verdict-driving fear in §2/§4, "neither (a) nor (b) solves the resource problem, only dropping the browser does," is empirically false for (b) on this hardware: production reference proof says the Chromium-based path *does* fit, with headroom to spare, running equivalent work (kiosk rendering, GPU accel, BT, OOM protection) to what this project needs.

**(2) Does this change whether rejecting React/Angular was right?** Yes, mostly, but one distinction from the original reasoning survives: §2's split between "monolith-as-supervision" and "monolith-as-resource-minimalism" was the right analytical move, and the user's stated reason ("lack of compute resources... avoid performance issues") was correctly read as the resource framing. What was wrong was the factual claim feeding that framing: "a browser-engine UI costs 150-250MB, an unacceptable fraction, and only Qt/QML avoids it." With the real number showing the full Chromium-based stock stack costs 152MB total, the resource-framing objection to React/Angular **no longer holds** on this hardware, it's been empirically cleared by the people who ship this exact device, running equivalent accelerated-rendering work. The *process-architecture* reasoning (QWebChannel in-process bridge vs. separate daemon+WebSocket) is a separate, narrower question that still stands on its own terms, but it is now moot for the top recommendation, because the stock reference implementation is architecture (b) (separate Chromium process + thin native supervisor), not (a) (QtWebEngine-embedded), and (b) is the one now carrying actual production proof. (a)'s one-time advantage, zero IPC hop for the bridge, remains theoretically true but is now a second-order optimization, not the deciding factor, since (a)'s QtWebEngine cross-compile path on this exact Yocto/BSP combination still has **zero** community precedent (unchanged from §1), whereas (b)-shaped plain-Chromium-kiosk is now **directly observed working** on this silicon today.

**(3) Revised final verdict: recommendation flips. Ship Chromium-kiosk + React/Angular, mirroring the stock reference implementation, not native Qt/QML, not QtWebEngine.**

**Concrete stack:**
- **Chromium kiosk**, invoked exactly as stock firmware does, `chromium-browser/chrome --kiosk --no-sandbox --in-process-gpu --app=file:///<path-to-build>/index.html`, rendering the production build of the React (or Angular) SPA served from local disk via `file://`, no static HTTP server needed (stock firmware already proves this works and avoids even the "fold the static server into the daemon" question §1(b) raised). `--in-process-gpu` is kept deliberately: it folds the GPU process into the main browser process (one fewer process and IPC hop than default multi-process Chromium) and is confirmed, by this same measurement, to drive the vendor Mali blob for Chromium's own rendering, reuse it rather than Chromium's default GPU-process split.
- **One thin native bridge/supervisor binary** (C or Rust; reuses the role `qt-superbird-app` plays in stock firmware) owning: evdev input (`event0`/`event1`/`event3` per `hardware-verification.md`), the BT-PAN client (`bluetoothd` stays a separate OS daemon regardless, BlueZ is never embeddable, per §3, unchanged), and the smart-home-controller WebSocket client (`research.md` §4.2). It exposes a small local WebSocket endpoint that the React/Angular app connects to on load, the simplest, most standard bridge for a web app, and the one already specified in `research.md` §4.3; CDP injection via the already-open remote-debugging port is a viable alternative worth prototyping (stock firmware leaves port 2222 open, suggesting the native launcher may already use it for something) but WebSocket is the safer default: well-understood, no dependency on devtools-protocol stability.
- Net process shape: `bluetoothd` + native bridge process + Chromium process tree (browser/zygote/renderer/audio-utility, same shape measured in the 152MB figure), structurally identical to architecture (b) from §1, with the static-file-server unit removed (via `file://` instead of HTTP) and `--in-process-gpu` applied, which stock firmware already validates as both correct and cheap.
- This is a **deliberate, explicit choice of (b) over (a)**, reversing §4's original ranking: (a)'s QtWebEngine path offered a marginal latency/process-count edge that was never measured and still has no cross-compile precedent on this Yocto layer set, while (b)-shaped plain Chromium kiosk is now the one with real production RAM numbers *on this exact device*. Pursuing (a) now would mean taking on unproven cross-compile risk to chase an optimization this measurement shows isn't needed.

**(4) Validated vs. still-estimated, stated explicitly, not overclaimed:**
- **Validated by production reference, on this exact SoC/RAM/firmware family:** Chromium-kiosk's total RAM cost including GPU-accelerated rendering via the vendor Mali blob (152MB for the *entire* stock stack, 323MB free afterward); that `bluetoothd` running concurrently doesn't materially change that picture (it's already included in the 152MB, the BT-PAN stack's incremental cost is therefore also empirically validated, not just estimated); that `--in-process-gpu` works and is the right flag to carry over; that `file://`-served local web apps work under this exact Chromium build/kiosk flags.
- **Still estimated, not yet measured, because nothing on stock firmware exercises them:** the user's own React/Angular bundle's RAM/CPU cost once built, stock firmware's reference web app is presumably simpler than a full smart-home dashboard with live-updating entity state, and bundle size/DOM complexity/animation load are unknowns that could push meaningfully above the stock app's footprint; the smart-home-controller WebSocket client's incremental cost (`research.md` §4.3 ballpark: 15 to 30MB for an asyncio Python daemon, smaller in C/Rust), not present in the stock measurement at all, since stock firmware has no backend client; and whether the native bridge binary's WebSocket-server role adds meaningfully to that estimate. None of these are RAM-risk-scale concerns given 323MB of validated headroom, but they are not proven zero, budget and measure them before final sign-off, same discipline §5 asked for originally, just against a much larger and now largely real (not estimated) headroom.

**Updated open items (supersedes §5 item 1, which is now answered; items 2-6 unchanged and still open):**
1. ~~Actual free RAM after boot~~, **answered**: 152MB used / 323MB available with the full reference stack running. No longer blocking.
2. Whether the project's own React/Angular build's steady-state + peak RAM, once it exists, stays comfortably inside the approximately 323MB headroom alongside the native bridge + smart-home-controller WebSocket client, now the actual open RAM question, replacing the old "is Chromium itself affordable at all" question.
3. Whether to use a local WebSocket bridge or CDP/remote-debugging injection for the native-binary-to-page channel, a design choice, not a feasibility unknown, worth a quick prototype of both before committing.
4. §5 items 2-6 from the original pass (yocto-superbird Panfrost/Ozone question, QtWebEngine cross-compile precedent, exact button scancodes, BT-PAN/NAP against stock BlueZ, Qt version availability) are **no longer load-bearing for the primary recommendation**, since this revision no longer recommends (a) or native Qt/QML as the shipped target, retained only as background context if a future override reconsiders either.

---

## 0. Restating the problem

The user added a fourth constraint on top of three already-settled ones:

1. (settled, prior turn) UI must be a React or Angular SPA with native hardware-input hooks.
2. (settled, two turns ago) Bluetooth is the *primary* transport to the smart-home controller; cable/USB is secondary.
3. (settled, earlier turn) The smart-home controller runs on a separate always-on host, nothing controller-side runs on-device.
4. (**new, this turn**) "Because of the lack of compute resources on this device, I would like to build an optimized monolith to avoid performance issues." 512MB RAM, Amlogic S905D2 (confirmed `research.md` §1.1), target: avoid perf issues.

These four constraints are not all mutually free. (1) and (4) are in tension, and the task brief is explicit that papering over that tension with "it depends" is not acceptable. This document resolves it with a concrete pick.

**Assumptions made explicit (stated or implied by the brief, surfaced for challenge):**
- "Monolith" is being used loosely by the user to mean *"one supervised thing, minimal moving parts, minimal RAM overhead from architecture itself"*, not literally "single OS process with zero subprocesses," because zero subprocesses is not achievable by any browser-engine-based design (see §1). I treat "monolith" as "minimize process count and inter-process hops," not as a literal single-PID requirement, and say so rather than silently redefining the user's word.
- "Avoid performance issues" is implicitly about *this device's* perf envelope (smooth touch/knob response, UI that doesn't visibly lag), not about absolute feature completeness.
- The smart-home controller host and network connectivity (BT-PAN or cable) are out of scope for this document, already covered in `research.md` §3 to §5.
- 512MB is *total* RAM, not available-after-boot RAM, this is an unverified gap, flagged explicitly in §5.

---

## 1. The three candidate architectures, elaborated

### (a) QtWebEngine native shell + QWebChannel bridge

**Process/thread model, concretely:**
QtWebEngine is not "a browser embedded as a library you call into once," it is Qt's wrapper around a nearly-full Chromium, and it reproduces Chromium's multi-process model *underneath* the single Qt application binary:
- 1x Qt application process (your C++ binary), this is "the app" from systemd's point of view, one supervised unit, one PID at the top.
- Chromium's own internal process split still happens **inside that process tree**, spawned by QtWebEngine as children: typically a GPU process, a renderer process per `QWebEngineView` (one SPA = one renderer, so this stays at *one* renderer in our case, not N), a network service process, and often a zygote/sandbox helper. Expect **4 to 6 OS processes total**, not 1, even though your own code and systemd see "one service."
- The evdev input reader and the smart-home-controller BT/network client can run as **threads inside the same Qt process** (Qt's event loop + `QThread`, or `asyncio`-in-a-thread via a bridge), feeding into the SPA via `QWebChannel`'s in-process JS-to-C++ object exposure, no socket, no serialization boundary, no separate daemon.

**RAM budget (reasoned estimate, confidence: medium, no device-specific measurement exists per `webapp-feasibility.md` §1.3/§3's explicit gap):**

| Component | Estimate | Basis |
|---|---|---|
| Kernel + Buildroot/BusyBox base | ~25-40MB | Typical for a stripped embedded Buildroot image at this kernel version (4.9-stock-era or mainline v7.x-yocto); not measured on this unit |
| BlueZ stack (`bluetoothd` + kernel BT modules) | ~5-10MB | Standard BlueZ userspace footprint |
| QtWebEngine core (GPU + renderer + network processes combined) | ~150-250MB | Extrapolated from CEF's approximately 70MB-for-simple-page-on-500MB-VM datapoint (`webapp-feasibility.md` §3a) scaled up for QtWebEngine's heavier Qt scaffolding + the Qt app binary itself |
| Your C++ shell code + evdev/BT/controller threads | ~10-20MB | Thin logic layer, no heavy runtime |
| **Total** | **~190-320MB**, leaving **~190-320MB headroom on a 512MB device before accounting for anything the bootloader/vendor reserves (framebuffer, CMA for the Mali blob, etc.)** | |

**Pros:** Single supervised systemd unit at the top; in-process JS-to-native bridge (no IPC hop for input events, lowest input latency of the three web-UI options); inherits the **confirmed-working** EGLFS+vendor-Mali-blob rendering path that stock firmware already proves works on this exact hardware (`hardware-verification.md`: `/dev/mali0` + `/dev/dri/card0` both live), this is the strongest point in (a)'s favor, because it's the only React/Angular-capable option with a *directly confirmed, not inferred* GPU-access path on *this physical unit's* firmware family. **[kept for history — this claim is known-inaccurate per later review: QtWebEngine's GPU work runs in-process by default per Qt's own documentation, it is not spawned as a separate GPU process the way this superseded section's process-count table above implies, and this section never evaluated Qt's commercial licensing cost for shipping QtWebEngine/Qt Quick in a product.]**

**Cons:** Still spawns 4 to 6 Chromium-lineage subprocesses regardless of app-code structure, "monolith" is true only at the supervision/IPC layer, not at the OS-process-count layer; zero community precedent on this exact device (`webapp-feasibility.md` §1.3); requires Qt/C++ cross-compilation via Yocto `populate_sdk` (solved mechanism, real time cost); RAM estimate is the least-confident number in this document because no CEF/QtWebEngine memory figure for *this* kernel+Mali-blob combo exists anywhere in the prior research.

**New hardware facts' effect on (a):**
- `/dev/mali0` + `/dev/dri/card0` both confirmed live via the **vendor blob**, not Panfrost (`hardware-verification.md` GPU section), this is unambiguously good news for (a) specifically, because EGLFS-via-vendor-blob is *exactly* the rendering path Qt/QtWebEngine would use, and it's now proven live on real hardware rather than theoretically available. This closes most of `webapp-feasibility.md`'s §1.2 open question *for the EGLFS path specifically* (it does not resolve whether Chromium's Ozone/DRM path inside QtWebEngine negotiates with the vendor blob cleanly, that remains unverified, see §5).
- REL_HWHEEL knob events (not discrete KEY_LEFT/RIGHT) are actually a *better* fit for (a) than for a naive design: Qt's native evdev handling or a small C++ reader can consume `REL_HWHEEL` deltas directly and expose a clean "knob delta" method via `QWebChannel` to the SPA, the SPA never has to know or care about raw Linux input semantics. Any of the three architectures handle this equally well in the end, since it's a bridge-layer translation problem, not an architecture-level one, noted here because (a)'s in-process bridge is the cleanest place to do that translation with zero serialization overhead.
- I2C touchscreen: no differential impact between architectures, it's a standard evdev multitouch device regardless of bus; QtWebEngine (like Chromium) expects normalized touch events from the platform input plugin either way.

### (b) Chromium kiosk + separately-served SPA + WebSocket bridge daemon

**Process/thread model, concretely:**
This is the architecture furthest from "monolith," structurally:
- 1x full Chromium kiosk process tree (browser process + GPU process + renderer process + network service, same 4-to-6-process Chromium shape as (a), but now as a **standalone browser**, not embedded in your own app binary), supervised as its own systemd unit, separate from your code entirely.
- 1x separate bridge daemon (Python per `research.md` §4.3), a **second** independently-supervised systemd unit, running the evdev reader + BT/controller client logic.
- 1x static file server (BusyBox httpd or Python `http.server`) serving the SPA build, arguably a **third** unit, though trivial enough to fold into the bridge daemon's own process if using Python (`aiohttp` can serve static files and run the WebSocket endpoint in the same process, cutting this back to two units).
- IPC: a local WebSocket (`ws://localhost:PORT`) between the browser-tab JS and the bridge daemon, this is a real network-stack round trip (loopback TCP), not a zero-cost in-process call. Latency is negligible in absolute terms (sub-millisecond on loopback) but it is an extra moving part, an extra thing that can be down/misconfigured/racing-at-boot, and an extra RAM-resident listener socket + TCP buffers, however small.

**RAM budget (reasoned estimate):**

| Component | Estimate |
|---|---|
| Kernel + BusyBox base | ~25-40MB |
| BlueZ stack | ~5-10MB |
| Chromium kiosk process tree | ~150-250MB (same order as (a)'s QtWebEngine core, it's the same Chromium engine, just not wrapped in Qt; arguably *slightly* lighter without Qt's own scaffolding on top, but no measured number exists for either on this hardware) |
| Bridge daemon (Python, evdev+BT+controller+WS server) | ~15-30MB (asyncio Python daemon, per `research.md` §4.3's "tens of MB" estimate) |
| Static file server (if separate from daemon) | ~2-5MB, or ~0 if folded into the daemon |
| **Total** | **~195-330MB**, comparable total RAM to (a), but spread across **2-3 independently-supervised units instead of 1** |

**Pros:** Fastest path to a working prototype, reuses `yocto-superbird`'s already-shipped `superbird-kiosk-*-image` template directly, reuses the already-specified Python daemon design verbatim, zero Qt/C++ work, standard web-dev skills only (`webapp-feasibility.md` §3.1's own "prototype first" recommendation).

**Cons:** This is the *opposite* of what the user is now asking for. It is two-to-three separately supervised OS services instead of one, with a loopback-network IPC hop between the input layer and the UI layer that (a) doesn't have, and the total RAM footprint is not meaningfully smaller than (a), you pay the same Chromium-process-tree cost either way, and gain nothing but *lose* the single-unit supervision story. The only genuine advantage over (a) is development speed/risk, not runtime resource efficiency.

**New hardware facts' effect on (b):**
- GPU: same open question as before for the *browser's own* GPU path. `hardware-verification.md` confirms the vendor Mali blob is live on *this* stock-ish firmware, but says nothing about whether `yocto-superbird`'s *separate* mainline-kernel-based Chromium build (a different firmware image entirely, per `hardware-verification.md`'s explicit note that Panfrost vs. vendor-blob is a *firmware choice*, not a property of the silicon) gets Chromium's Ozone/DRM backend talking to Panfrost cleanly. This remains **unverified** exactly as `webapp-feasibility.md` §1.2 already flagged, and the new on-device evidence does not close this gap for (b) the way it partially does for (a), because (b) would run on a *different* OS image (yocto-superbird's kiosk build) than the one actually inspected in `hardware-verification.md` (stock-ish firmware). This is a meaningful asymmetry the user should know: **(a) has a firmer (though still partial) GPU-path confirmation than (b) does, precisely because the live hardware check was run against stock firmware's Qt/EGLFS/vendor-blob stack, not against yocto-superbird's Chromium/mainline-kernel stack.**
- REL_HWHEEL + WebSocket bridge: the bridge daemon must translate raw `REL_HWHEEL` deltas into whatever message schema the SPA's WebSocket client expects, same translation work as (a), just serialized to JSON over loopback instead of called in-process. Marginal CPU/RAM cost, not a feasibility blocker.
- BT already live on stock firmware doesn't actually help (b) directly, because (b) requires re-flashing to `yocto-superbird` for the Chromium kiosk image anyway (stock firmware has no Chromium at all, confirmed `hardware-verification.md` line 8-9/24-25), so (b) loses the "BT already works, no re-flash needed" advantage that a non-rendering design could have kept.

### (c) No on-device rendering: pure input satellite

**Process/thread model, concretely:**
- 1x Python asyncio daemon: evdev reader + BT/controller client, exactly as specified in `research.md` §4.3, running as the device's only meaningful application process.
- Optionally, a trivial native Qt/EGLFS status screen (not a React/Angular SPA) as a second lightweight process if *some* on-device visual feedback is wanted, this reuses the confirmed-working stock rendering path with zero browser-engine involvement.
- The React/Angular dashboard itself runs entirely off-device (on the controller host or any other always-on machine), it is **not part of this device's process model at all**.

**RAM budget:** trivially fine, tens of MB total, per `research.md` §4.3 and `webapp-feasibility.md` §3's own estimate. Over 450MB headroom.

**Pros:** True monolith in the strictest sense, one process, no browser engine, no subprocess tree, lowest possible resource usage by a wide margin, zero new rendering-stack risk, and it's the *only* option with directly-confirmed production precedent on this device family (`superbird-debian-kiosk`'s shipped button-to-backend-service feature, `research.md` §4.4).

**Cons:** **Does not deliver a React/Angular UI on the device at all.** This directly fails the user's standing requirement from two turns ago. It is the correct fallback if both (a) and (b) prove infeasible in practice, not a competing answer to the question actually asked.

**New hardware facts' effect on (c):** Entirely positive and low-risk, BT already live on stock firmware (`hardware-verification.md`) means (c) could in principle be prototyped **without any re-flash at all**, against the exact firmware already on the unit. REL_HWHEEL and I2C touch are straightforward evdev reads either way. This is the only architecture that doesn't need to wait on any unresolved GPU/Chromium question.

---

## 2. Resolving the monolith-vs-React/Angular tension directly

The brief frames this as an either/or: (a) show how QtWebEngine can be "monolithic enough," or (b) admit React/Angular must be dropped for native Qt/QML.

**Steelmanning "QtWebEngine is monolithic enough" (the case for keeping (a) and calling it a monolith):**
It's true that (a) collapses supervision to one systemd unit and removes the loopback-WebSocket IPC hop that (b) requires. If "monolith" means "one thing ops has to supervise, one thing that can crash and be restarted as a unit, no network-stack hop between input and UI," (a) genuinely delivers that, and does so while reusing the one GPU-rendering path that's actually been seen working on real hardware (vendor Mali blob via EGLFS). That is a real, non-hand-wavy property, and it is the honest version of "more monolithic than (b)," not a dodge.

**Steelmanning the opposing view (the case that this is still not what "optimized monolith to avoid performance issues" means):**
The user's stated *reason* for wanting a monolith is explicitly **performance on a RAM-constrained device**, not organizational/supervision tidiness. On that specific axis, RAM and CPU actually consumed, (a) and (b) are **the same order of magnitude** (§1's tables: ~190-320MB either way). QtWebEngine does not make Chromium smaller; it just moves Chromium's supervision inside your binary. If the user's real pain point is "512MB is tight and I don't want a 200-300MB rendering engine eating most of it," **neither (a) nor (b) solves that**, only dropping the browser engine entirely (option (c), or its React/Angular-abandoning cousin, native Qt/QML) actually reduces the RAM number. Calling (a) "the monolith" solves the *supervision/IPC* framing of "monolith" but does **not** solve the *resource* framing of "monolith," and the user's own stated justification ("lack of compute resources... to avoid performance issues") is unambiguously the resource framing, not the supervision framing.

**Verdict, I am not going to split this down the middle:**

The user is using "monolith" to mean two different things without realizing it, and only one of those meanings is actually in tension with React/Angular:
- **Monolith-as-supervision-model** (one process, in-process bridge, no IPC hop): React/Angular via QtWebEngine achieves this. No conflict here.
- **Monolith-as-resource-minimalism** (small RAM/CPU footprint because there's little running): React/Angular, via *any* browser-engine path, cannot achieve this, because a full Chromium-lineage rendering engine is the dominant cost on this device regardless of how it's packaged. This is the real conflict, and it is **not resolvable** by choosing between (a) and (b), both carry the same approximately 150-250MB Chromium tax. The only way to get genuine resource-minimalism is to **not run a browser engine at all.**

Given the user explicitly named "lack of compute resources" and "performance issues" as the motivating reason, not "I want fewer systemd units", **the resource-framing is what they actually care about**, even though they phrased the ask using "monolith," a word that technically only commits to the supervision-framing. I am correcting the premise here rather than silently answering the supervision question: **if RAM/CPU headroom on a 512MB device is the real goal, the only architecture that delivers it is native Qt/QML, not QtWebEngine-wrapped React/Angular.** QtWebEngine is a legitimate, defensible choice if the user is willing to spend approximately 200-300MB of a 512MB budget on web-rendering in exchange for keeping React/Angular, but that is a conscious tradeoff to make explicitly, not something "monolith" quietly papers over.

**What would change this conclusion:** if the actual free RAM after boot (§5, unmeasured) turns out to be, say, 400MB+ rather than something closer to 300MB, the approximately 200-300MB Chromium tax stops being "most of the budget" and starts being "a large but survivable chunk," which would make (a) a comfortably defensible choice rather than a risky one. This number is the single biggest unmeasured fact gating this whole decision, see §5.

---

## 3. Where BT and smart-home-controller client logic live, and how that interacts with the monolith goal

Per `research.md` §3.3/§5, the chosen design is: BT-PAN/BNEP as primary transport, USB-CDC-NCM cable as secondary, with the application protocol (the controller's WebSocket API, `research.md` §4.2) running identically over either transport once up. The question here is purely **where that logic executes relative to the rendering layer**, for each of the three architectures:

| Architecture | BT-PAN stack (`bluetoothd`, `bnep0`) | Smart-home-controller WebSocket client + link-monitor | Relationship to rendering process |
|---|---|---|---|
| (a) QtWebEngine | Separate OS daemon (`bluetoothd`), always a separate process regardless of architecture, BlueZ is not embeddable | **In-process**, as a Qt thread/`QThread` inside the same binary as the rendering shell, exposed to the SPA via `QWebChannel` for state push (e.g. "light turned on elsewhere") | Tightest coupling of the three, one process owns input, controller client, and UI exposure |
| (b) Chromium kiosk | Separate OS daemon (`bluetoothd`), same as (a) | **In the bridge daemon**, a process fully separate from the Chromium tree, talking to the SPA only via the local WebSocket | Loosest coupling, controller client and UI are two independently-restartable processes connected by loopback IPC |
| (c) input satellite | Separate OS daemon (`bluetoothd`), same as both | **In the (only) application process** | Trivially monolithic, there's only one app process, period |

`bluetoothd` itself is never foldable into any of these application processes, it's BlueZ's own D-Bus-based system daemon, and nothing in any community project reviewed (`research.md` §3) runs it embedded. So **zero architecture here achieves a literal single-process system**, all three run at minimum `bluetoothd` + one application process, and (a)/(b) additionally run the Chromium-lineage process tree. This reinforces §2's conclusion: the "how many processes" question has a floor of 2 (kernel services aside) regardless of UI choice, and the real differentiator between the three options is the approximately 200-300MB rendering-engine tax, not the BT/controller-client placement, which is cheap and flexible in all three.

The link-monitor logic (`research.md` §5.3, watching `usb0`/`bnep0` carrier state to pick transport) is a few-KB-of-state concern and goes wherever the controller client goes in each case: in-process thread for (a)/(c), in the bridge daemon for (b).

---

## 4. Final recommendation

**Primary recommendation: drop React/Angular for this device. Build native Qt/QML, EGLFS, directly against the confirmed vendor Mali blob, no browser engine anywhere on-device.**

Named stack: **Qt 6.x (or 5.15 LTS if 6.x cross-compile tooling for this exact Yocto/BSP combination proves immature) + QML for the UI + a single C++ (or Rust-via-`qmetaobject`/`cxx-qt`, if the team prefers Rust's memory-safety for the BT/network layer) application binary** that:
- Renders the knob/button/touch-driven dashboard natively via QML, running on the already-confirmed-working EGLFS + vendor-Mali-blob path (`hardware-verification.md`).
- Reads `/dev/input/event0` (buttons, `gpio-keys`), `/dev/input/event1` (knob, `REL_HWHEEL` on `rotary@0`), and `/dev/input/event3` (touch, `tlsc6x_dbg` over I2C) directly via Qt's own evdev input plugin or a thin custom reader thread, no separate evdev daemon needed.
- Holds the BT-PAN/smart-home-controller WebSocket client logic in a `QThread` or Qt's async networking primitives (`QWebSocket`), in the same binary.
- Runs as exactly **one supervised systemd unit** (plus the unavoidable, separate `bluetoothd`), with RAM usage in the tens-of-MB range, the same order of magnitude as the pure input-satellite option (c), but with a real on-device touch UI.

This is a genuine single-process (modulo `bluetoothd`) monolith, it is not a Chromium-lineage process tree wearing a disguise, and it reuses the exact rendering path (`EGLFS` + vendor Mali blob) that is **directly confirmed live on this physical unit's firmware today**, the strongest hardware-evidence basis of any option discussed in this document. This satisfies the resource-framing of "monolith" that §2 concluded is the user's real motivating concern, at the direct cost of the originally-requested React/Angular toolchain.

**Justification tied to confirmed constraints:**
- 512MB total RAM with no measured free-after-boot figure (§5) means the approximately 150-250MB floor any Chromium-lineage design imposes is the single largest risk factor in this whole project; native Qt/QML removes that risk entirely rather than budgeting around it.
- The GPU path this recommendation relies on is not a projection from a different SoC family (`webapp-feasibility.md`'s Manjaro/Panfrost hobbyist report), it is the *exact* `/dev/mali0`/`/dev/dri/card0` combination directly read off *this* physical device, today (`hardware-verification.md`).
- REL_HWHEEL and the I2C touch controller are both standard Qt/evdev-input-plugin-compatible event sources; no special handling beyond what Qt's embedded input stack already does for any Linux evdev device.
- BT already enumerated (`hci0`) on current stock-ish firmware means early BT-PAN prototyping (`research.md` §3's primary transport choice) can start **before** committing to any re-flash, directly reducing project risk/sequencing cost regardless of which final OS image is chosen.

**If the user is unwilling to give up React/Angular** (a legitimate call, team skills, ecosystem reuse, dashboard-card compatibility with the chosen smart-home controller are all real reasons to prefer it), the fallback recommendation, in order, is:
1. **(a) QtWebEngine native shell + QWebChannel**, accepting the approximately 150-250MB Chromium tax as a measured, bounded cost against the confirmed-working EGLFS path, only if §5's RAM-after-boot measurement comes back comfortably above that tax (rule of thumb: need to see 350MB+ free after boot before this stops being a real risk). This is strictly preferable to (b) given identical RAM cost but tighter supervision and lower input-to-UI latency.
2. **(b) Chromium kiosk + separate daemon** only as a fast-prototyping step to empirically answer the open `chrome://gpu` question on `yocto-superbird` specifically (`webapp-feasibility.md` §1.2's own recommendation), not as the shipped production architecture, because it offers no resource advantage over (a) while costing the single-process supervision story entirely.
3. **(c) input satellite** remains the safety net if even (a) proves infeasible (e.g., QtWebEngine simply won't build/run against this exact Yocto layer set), it is zero-risk but does not deliver an on-device web UI, so it is a fallback, not a target.

This is a decisive pick, not a menu: **ship native Qt/QML unless the user explicitly overrides on the grounds of team skill/ecosystem reuse, in which case ship (a), never (b) as a production target.**

---

## 5. Items requiring on-device verification before implementation starts

1. **Actual free RAM after boot.** This is the single most load-bearing unmeasured number in this entire document, §2's and §4's conclusions both hinge on how much of the 512MB total is actually available to application code after the kernel, BusyBox/init, and any vendor-reserved framebuffer/CMA allocations for the Mali GPU are accounted for. **Next concrete step: `adb shell free -m` (and `cat /proc/meminfo` for the CMA/reserved breakdown) against the live unit.** Until this is measured, every RAM-budget table in §1 is a reasoned estimate, not a fact.
2. Whether `yocto-superbird`'s Chromium kiosk build specifically gets Ozone/DRM talking to Panfrost (relevant only if (b) or a Chromium-based option is pursued despite §4's recommendation), `chrome://gpu` on a real `superbird-kiosk-dev-image` flash, per `webapp-feasibility.md` §1.2/§3.1.
3. Whether QtWebEngine cross-compiles and *runs* against `yocto-superbird`'s `populate_sdk` output at all on this exact board, zero community precedent exists (`webapp-feasibility.md` §1.3); needed only if the user overrides §4's primary recommendation in favor of (a).
4. Exact scancode-to-physical-button mapping and whether knob-click lives in `event0`'s bitmap, `hardware-verification.md` confirms the event *nodes and types* but not the live-press scancode table; needed for the input-reader layer regardless of which architecture is chosen.
5. Whether BT-PAN/NAP actually comes up cleanly against *this* stock-ish firmware's BlueZ version (vs. requiring a re-flash to `yocto-superbird`), now that `hci0` is confirmed live, directly actionable next step per `hardware-verification.md`'s own closing note, independent of the UI-architecture decision in this document.
6. Qt version availability (5.15 LTS vs 6.x) in whichever Yocto layer set is ultimately used for the native-Qt/QML build this document recommends, not yet checked against `yocto-superbird`'s specific recipe set.

None of these block writing the Qt/QML application skeleton or the BT/smart-home-controller client logic in parallel with verification, they block final sign-off on whether the fallback-to-(a) path stays open, and whether the native recommendation's RAM headroom claim in §4 holds up numerically.
