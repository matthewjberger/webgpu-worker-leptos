# Architecture

This document explains how `webgpu-worker-leptos` is put together: the crate
layout, the thread split, the message protocol, the startup handshake, and the
per-frame data flow. File paths are relative to the repository root, and key
functions and types are named so you can grep for them.

## The one idea

Every piece of GPU work runs on a **web worker**, never on the browser's main
thread. The worker owns an `OffscreenCanvas`, holds the entire `wgpu` device and
render loop, and draws a lit, spinning cube. The main thread runs a **Leptos**
(Rust → wasm) UI whose only jobs are to transfer the canvas to the worker once,
forward input events, and display stats streamed back from the worker.

The payoff is demonstrated by the "Jam main thread for 3 s" button: it freezes
the main thread in a busy-loop, and the cube keeps spinning at full framerate
because the render loop lives on another thread entirely.

This is a Rust-on-both-sides reimplementation of the TypeScript +
[Comlink](https://github.com/GoogleChromeLabs/comlink) original,
[webgpu-worker](https://github.com/matthewjberger/webgpu-worker). Comlink's
transparent proxy is replaced by an explicit `postMessage` protocol whose types
are defined once in a shared crate, so the two sides cannot disagree on the wire
format.

## Workspace layout

A Cargo workspace (`Cargo.toml`) with three member crates plus the root crate:

| Crate | Path | Runs on | Role |
|---|---|---|---|
| `protocol` | `protocol/` | both | The shared message and data types. The wire-format contract. |
| `renderer` | `renderer/` | worker | Pure `wgpu`. No JS bindings, no windowing. Owns `WgpuApp`. |
| `worker` | `worker/` | worker | The wasm module inside the web worker. Owns the render loop and the message handler. |
| root (`webgpu-worker-leptos`) | `src/` | main thread | The Leptos UI: control panel, event forwarding, stats display. |

Two of these compile to wasm independently:

- The **root crate** is built by Trunk into the page's wasm bundle.
- The **worker crate** is built separately (`cargo build -p worker` +
  `wasm-bindgen --target web` + `wasm-opt`) into `runtime/engine.js` and
  `runtime/engine_bg.wasm`.

They are two distinct wasm modules with two separate build steps. That is why
the `justfile` builds the worker explicitly before invoking Trunk — Trunk only
knows about the page.

`renderer` deliberately has no JavaScript dependency. It pulls in `wgpu`,
`bytemuck`, `nalgebra-glm`, and `web-time`, and could be linked into a native
binary unchanged. All the JS-facing glue lives in `worker` and the root crate.

## The thread split

```
┌─────────────────────────── MAIN THREAD ──────────────────────────────┐
│  Leptos wasm app  (src/)                                              │
│                                                                       │
│   src/main.rs    mount_to_body(App)                                   │
│   src/app.rs     UI, pointer/wheel handlers, rAF input loop, signals  │
│   src/bridge.rs  postMessage wrapper + stats request/response         │
│                                                                       │
│   transfer_control_to_offscreen()  ── canvas (ownership) ──┐          │
│   worker.post_message(ClientMessage) ──────────────────────┼────────▶ │
│   worker.onmessage(WorkerMessage)    ◀─────────────────────┘          │
└───────────────────────────────────────────────────────────────────────┘
                              │  serde-serialized enums over postMessage
                              ▼
┌─────────────────────────── WEB WORKER ───────────────────────────────┐
│  runtime/worker.js   tiny JS bootstrap shim (buffers early messages)  │
│       └── loads engine.js (wasm-bindgen glue) + engine_bg.wasm        │
│                                                                       │
│  worker crate  (worker/src/lib.rs)                                    │
│   • #[wasm_bindgen(start)] installs scope.onmessage                   │
│   • decodes ClientMessage, dispatches to WgpuApp                      │
│   • requestAnimationFrame render loop                                 │
│                                                                       │
│  renderer crate  (renderer/src/lib.rs)                                │
│   • WgpuApp: Gpu, Scene, OrbitCamera, FrameStats                      │
│   • surface straight from OffscreenCanvas, inline WGSL Lambert shader │
└───────────────────────────────────────────────────────────────────────┘
```

After the one-time canvas transfer, the main thread can never draw to the
canvas again — ownership of the rendering surface has moved to the worker. From
that point the two threads communicate only by `postMessage`.

## The message protocol

`protocol/src/lib.rs` defines two enums and the data structs they carry. Both
sides depend on this crate, so a message that serializes on one side always
deserializes on the other.

```rust
// page → worker
enum ClientMessage {
    Init { width, height },          // sent once, with the OffscreenCanvas in the transfer list
    Resize { width, height },
    SetSpeed { speed },
    SetColor { red, green, blue },
    Orbit { yaw, pitch },            // accumulated input deltas
    Zoom { amount },
    Pick { x, y },                   // click position in normalized device coords
    StatsRequest { id },             // the one request that wants a reply
}

// worker → page
enum WorkerMessage {
    Ready { info: AdapterInfo, context: String },   // GPU is up; names the adapter + worker scope
    Stats { stats: Stats },                          // pushed ~4×/sec
    StatsReply { id, stats },                        // answer to StatsRequest
    Picked { hit: Option<PickResult> },              // ray-cast result
}
```

There are two interaction styles, matching the README's description:

- **One-way streams** for everything that does not need an answer: orbit, zoom,
  speed, color, resize, and the periodic `Stats` and `Picked` pushes. Fire and
  forget.
- **One request/response round-trip**: `StatsRequest { id }` →
  `StatsReply { id, stats }`. The `id` correlates a reply with its request
  (Comlink would have done this implicitly via a proxied method return). This is
  the only place the page blocks on a worker answer, and it exists for the jam
  measurement.

## Startup handshake

The ordering here matters, because a worker's message queue goes live before its
wasm has loaded.

1. **Trunk builds the page** (`index.html`). It bundles the root crate to wasm
   and `copy-dir`s the prebuilt `runtime/` folder (`worker.js`, `engine.js`,
   `engine_bg.wasm`) into `dist/`.
2. **The Leptos app mounts** (`src/main.rs`, `mount_to_body(App)`).
3. **A setup `Effect` fires once the `<canvas>` exists** (in `App`, `src/app.rs`).
   It:
   - sizes the canvas backing store by `device_pixel_ratio` for crisp output,
   - calls `canvas.transfer_control_to_offscreen()` — the pivotal handoff,
   - spawns the worker as an **ES module** from `runtime/worker.js`
     (`WorkerType::Module`),
   - wires `worker.onmessage` to decode `WorkerMessage` and update Leptos
     signals,
   - builds a `Bridge` and sends `Init`, **with the `OffscreenCanvas` in the
     transfer list** (`Bridge::send_init`, `src/bridge.rs`). This is a
     zero-copy ownership transfer via `post_message_with_transfer`, not a clone.
4. **The JS shim absorbs the race** (`runtime/worker.js`). A worker starts
   queueing messages the instant its top-level script finishes, but the Rust
   `onmessage` handler is not installed until `init()` resolves. The `Init`
   message (carrying the canvas) can arrive in that gap. So the shim registers a
   synchronous handler that buffers events, then replays them once the wasm has
   installed the real handler:

   ```js
   const buffered = [];
   self.onmessage = (event) => buffered.push(event);
   init().then(() => { for (const e of buffered) self.onmessage(e); });
   ```
5. **The worker initializes the GPU** (`handle_message`, `worker/src/lib.rs`). On
   `Init` it reads the transferred canvas off the message, `spawn_local`s the async
   `WgpuApp::create`, posts `Ready` (with adapter/backend names and the worker
   scope name), stores the app, and starts the render loop.

## Inside the worker

`worker/src/lib.rs` is the message pump and loop driver:

- `start()` grabs the `DedicatedWorkerGlobalScope`, creates an empty
  `app_slot: Rc<RefCell<Option<WgpuApp>>>` (empty until GPU init finishes), and
  installs the real `onmessage`.
- `handle_message` pulls the `message` field off the envelope object,
  deserializes a `ClientMessage`, and dispatches: `Init` builds the app and
  starts the loop; `Resize`/`SetSpeed`/`SetColor`/`Orbit`/`Zoom` mutate the app;
  `Pick` runs the ray cast and posts `Picked`; `StatsRequest` posts a
  `StatsReply`.
- `start_render_loop` builds the standard wasm `requestAnimationFrame`
  self-rescheduling closure (a `Closure` stored in `Rc<RefCell<Option<…>>>` so
  it can re-request itself). Each frame it calls `app.update()` and, at most
  every 250 ms, pushes a `Stats` message. Throttling the push keeps `postMessage`
  traffic low while the render loop itself runs every frame.

The envelope shape matters: messages are sent as a plain JS object with a
`message` field (the serialized enum) and, for `Init` only, a `canvas` field
(the transferred `OffscreenCanvas`). Both sides agree on these field names
(`Bridge::send`/`send_init` in `src/bridge.rs`, `handle_message` in
`worker/src/lib.rs`).

## The renderer

`renderer/src/lib.rs` is hand-written `wgpu` with no engine and no windowing
layer. `WgpuApp` holds a `Gpu`, a depth texture view, a `Scene`, `Controls`, an
`OrbitCamera`, and `FrameStats`.

**Surface creation** (`Gpu::new_async`) is the key simplification. The surface
comes straight from the transferred canvas:

```rust
instance.create_surface(wgpu::SurfaceTarget::OffscreenCanvas(canvas))
```

Because `wgpu` has a first-class offscreen-canvas surface target, there is no
`winit`, no `raw-window-handle`, and none of the `unsafe impl Send + Sync`
window-handle wrapper that an engine-based worker port would need. It then
requests an adapter and device, and picks the first **non-sRGB** surface format
so the shader can do its own sRGB conversion.

**The scene** (`Scene`) is a 24-vertex / 36-index cube built per face with
outward normals (`build_cube`), one uniform buffer, and one render pipeline
compiled from inline WGSL. The uniform (`UniformBuffer`) carries `mvp`, `model`,
`tint`, and `hit_point`.

**Per-frame update** (`Scene::update`) builds:

- a left-handed perspective projection with `perspective_lh_zo` — the `_zo`
  suffix is the 0..1 depth range WebGPU expects, not OpenGL's -1..1,
- a `look_at_lh` view from the orbit camera,
- a model matrix spinning on two axes,

uploads them via `queue.write_buffer`, and advances the fps window.

**Render** (`WgpuApp::render`) is a single pass: clear to dark gray, depth
attachment (`Depth32Float`, `Less` compare), draw the indexed cube, submit,
present. A lost or outdated surface triggers a reconfigure and a skipped frame.

**The WGSL shader** (`SHADER_SOURCE`) does Lambert diffuse shading
against a fixed light direction, with a `0.25 + 0.75 * diffuse` floor so faces
never go fully black. It applies the tint in linear space and, when
`hit_point.w > 0.5`, paints an orange marker blob around the picked point using
`smoothstep` on distance. Because the surface format is non-sRGB, the shader
converts sRGB↔linear by hand.

## Input handling and coalescing

Raw pointer and wheel events do not message the worker directly. They accumulate
into a shared `DragState`: `pending_yaw`, `pending_pitch`, `pending_zoom`. A
separate `requestAnimationFrame` loop on the *main* thread (set up in the `App`
`Effect`) runs once per frame and, if there is pending motion, sends a single
`Orbit` and/or `Zoom` message and zeroes the accumulators. This caps
worker-bound input traffic at one message per frame regardless of how many raw
events fired.

That same main-thread loop also increments a `heartbeat` signal each frame. The
heartbeat is the visible proof of main-thread liveness: it freezes during the
jam test while the worker's fps counter keeps climbing.

A `ResizeObserver` watches the canvas and sends DPR-scaled `Resize` messages on
layout changes.

## Picking

Clicking the cube to mark a spot is a CPU ray cast in Rust — no GPU readback.

1. **Main thread** (`on_pointerup` in `src/app.rs`): a pointer-up that moved
   less than 4 px is treated as a click. The click position is converted to
   normalized device coordinates (-1..1, Y flipped) and sent as `Pick { x, y }`.
2. **Worker** (`Scene::pick` in `renderer/src/lib.rs`): unproject the NDC point
   through `inverse(view_proj)` to get near/far world points, transform the ray
   into the cube's local space via `inverse(model)`, then run a slab ray/AABB
   intersection (`ray_cube_hit`) against the half-extent cube. It returns the hit
   point and which face was hit (`+X`, `-Y`, …).
3. The hit point is written into `hit_point` so the shader draws the marker, and
   a `Picked` message updates the "Picked: …" text on the page.

## The jam demonstration

`on_jam` (in `src/app.rs`) is the whole point made measurable:

1. `request_stats().await` reads the worker's current frame count (`before`).
2. The main thread busy-loops for 3 seconds:
   `while performance.now() - start < 3000.0 {}`. The page is genuinely frozen —
   the heartbeat stops, the UI is unresponsive.
3. `request_stats().await` reads the count again (`after`).
4. It reports how many frames `wgpu` advanced during the freeze
   (`after.frames - before.frames`), which is ~180 at 60 fps — proof the render
   loop never stalled.

The await is implemented in `Bridge::request_stats` (`src/bridge.rs`). Since the
protocol is one-way by default, it manually reconstructs a request/response: it
creates a JS `Promise`, stashes the `resolve` function in a shared
`pending_stats` slot, sends `StatsRequest`, and the page's `worker.onmessage`
handler calls that stored `resolve` when the matching `StatsReply` arrives. This
is exactly the round-trip Comlink would have given for free.

## End-to-end data flow

A single frame, with the message direction marked:

```
MAIN THREAD                                    WEB WORKER
───────────                                    ──────────
pointer/wheel events
    │ accumulate into DragState
    ▼
rAF loop (once per frame)
    │ Orbit{yaw,pitch}, Zoom{amount}   ──────▶ handle_message → app.orbit / app.zoom
    │ heartbeat += 1
                                               rAF loop (once per frame)
                                                   │ app.update()
                                                   │   Scene::update → queue.write_buffer
                                                   │   render() → submit + present
                                                   │ every 250ms:
    fps / frames signals  ◀───── Stats{stats} ─────┘

click ── Pick{x,y} ─────────────────────────▶ Scene::pick (ray/AABB)
    pick text  ◀───────── Picked{hit} ─────────┘

jam button:
  request_stats() ── StatsRequest{id} ───────▶ app.stats()
              ◀──────── StatsReply{id,stats} ──┘
  busy-loop 3s  (worker keeps rendering throughout)
  request_stats() ── StatsRequest{id} ───────▶ app.stats()
              ◀──────── StatsReply{id,stats} ──┘
  report advanced frames
```

The two render loops are independent. The main thread's rAF loop exists only to
batch input and tick the heartbeat; the worker's rAF loop is what actually
renders. Blocking the former does nothing to the latter.

## Build pipeline

`just run` (`justfile`) runs three steps in order:

1. `worker` — compile the worker crate to `wasm32-unknown-unknown`, run
   `wasm-bindgen --target web` to emit `runtime/engine.js` and
   `engine_bg.wasm`, then `wasm-opt -Oz` to shrink it.
2. `tailwind` — generate `public/styles.css` from `public/input.css`.
3. `trunk serve` — build the main-thread Leptos app, copy `runtime/` into
   `dist/`, and serve at `http://127.0.0.1:8080`.

The worker wasm and the page wasm are built by two different toolchains (raw
`wasm-bindgen` for the worker, Trunk for the page), which is why the worker step
is split out rather than folded into Trunk.

Requires a browser with WebGPU and `OffscreenCanvas`-in-workers support
(Chromium 113+, Firefox 141+).

## Why it is shaped this way

- **Shared `protocol` crate** — the page and worker physically cannot disagree
  on the wire format, because the types are defined once and both depend on
  them. This replaces Comlink's typed proxy with a compile-time guarantee.
- **`renderer` has no JS dependency** — keeping the wgpu code free of
  `wasm-bindgen`/`web-sys` (apart from the one `OffscreenCanvas` type) makes it a
  normal Rust library that happens to be driven by the worker. The JS glue is
  isolated in `worker` and `src/`.
- **Surface from `OffscreenCanvas` directly** — no `winit`, no
  `raw-window-handle`, no unsafe window-handle wrapper. wgpu's offscreen-canvas
  surface target removes an entire layer that an engine-based port would carry.
- **Explicit request/response for exactly one round-trip** — only the stats poll
  needs an answer, so only it pays the correlation-id cost. Everything else is a
  cheap one-way stream.
</content>
</invoke>
