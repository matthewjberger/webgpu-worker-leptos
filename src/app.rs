use std::cell::RefCell;
use std::rc::Rc;

use leptos::html;
use leptos::prelude::*;
use protocol::{ClientMessage, WorkerMessage};
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Event, HtmlInputElement, MessageEvent, PointerEvent, ResizeObserver, WheelEvent, Worker,
    WorkerOptions, WorkerType,
};

use crate::bridge::Bridge;

type FrameLoop = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

#[derive(Default)]
struct DragState {
    pending_yaw: f32,
    pending_pitch: f32,
    pending_zoom: f32,
    dragging: bool,
    moved: f32,
    last_x: f32,
    last_y: f32,
}

#[component]
pub fn App() -> impl IntoView {
    let context = RwSignal::new("checking…".to_string());
    let adapter = RwSignal::new("…".to_string());
    let fps = RwSignal::new(0.0_f32);
    let frames = RwSignal::new(0.0_f64);
    let heartbeat = RwSignal::new(0_u64);
    let jammed = RwSignal::new(false);
    let speed = RwSignal::new(1.0_f32);
    let jam_result = RwSignal::new(String::new());
    let pick_text = RwSignal::new("click the cube to mark a spot".to_string());
    let grabbing = RwSignal::new(false);

    let canvas_ref = NodeRef::<html::Canvas>::new();
    let bridge_slot: Rc<RefCell<Option<Bridge>>> = Rc::new(RefCell::new(None));
    let drag = Rc::new(RefCell::new(DragState::default()));

    let setup_slot = bridge_slot.clone();
    let setup_drag = drag.clone();
    Effect::new(move |_| {
        let Some(canvas) = canvas_ref.get() else {
            return;
        };
        if setup_slot.borrow().is_some() {
            return;
        }

        let window = web_sys::window().unwrap();
        let dpr = window.device_pixel_ratio() as f32;
        let rect = canvas.get_bounding_client_rect();
        let width = rect.width() as f32 * dpr;
        let height = rect.height() as f32 * dpr;
        canvas.set_width(width as u32);
        canvas.set_height(height as u32);

        let offscreen = canvas
            .transfer_control_to_offscreen()
            .expect("failed to transfer canvas to offscreen");

        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = Worker::new_with_options("runtime/worker.js", &options)
            .expect("failed to spawn worker");

        let pending_stats: crate::bridge::StatsResolver = Rc::new(RefCell::new(None));

        let message_pending = pending_stats.clone();
        let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let Ok(message) = serde_wasm_bindgen::from_value::<WorkerMessage>(event.data()) else {
                return;
            };
            match message {
                WorkerMessage::Ready {
                    info,
                    context: scope,
                } => {
                    adapter.set(format!("{} ({})", info.adapter, info.backend));
                    context.set(format!("{scope} (off the main thread)"));
                }
                WorkerMessage::Stats { stats } => {
                    fps.set(stats.fps);
                    frames.set(stats.frames);
                }
                WorkerMessage::StatsReply { id: _, stats } => {
                    if let Some(resolve) = message_pending.borrow_mut().take()
                        && let Ok(value) = serde_wasm_bindgen::to_value(&stats)
                    {
                        let _ = resolve.call1(&JsValue::NULL, &value);
                    }
                }
                WorkerMessage::Picked { hit } => {
                    pick_text.set(match hit {
                        Some(hit) => {
                            format!("{} at ({:.2}, {:.2}, {:.2})", hit.face, hit.x, hit.y, hit.z)
                        }
                        None => "no hit".to_string(),
                    });
                }
            }
        });
        worker.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
        onmessage.forget();

        let bridge = Bridge::new(worker, pending_stats);
        bridge.send_init(offscreen, width, height);
        *setup_slot.borrow_mut() = Some(bridge.clone());

        let frame: FrameLoop = Rc::new(RefCell::new(None));
        let frame_handle = frame.clone();
        let loop_bridge = bridge.clone();
        let loop_drag = setup_drag.clone();
        let loop_window = window.clone();
        *frame.borrow_mut() = Some(Closure::<dyn FnMut()>::new(move || {
            heartbeat.update(|value| *value += 1);
            {
                let mut state = loop_drag.borrow_mut();
                if state.pending_yaw != 0.0 || state.pending_pitch != 0.0 {
                    loop_bridge.send(&ClientMessage::Orbit {
                        yaw: state.pending_yaw,
                        pitch: state.pending_pitch,
                    });
                    state.pending_yaw = 0.0;
                    state.pending_pitch = 0.0;
                }
                if state.pending_zoom != 0.0 {
                    loop_bridge.send(&ClientMessage::Zoom {
                        amount: state.pending_zoom,
                    });
                    state.pending_zoom = 0.0;
                }
            }
            if let Some(callback) = frame_handle.borrow().as_ref() {
                let _ = loop_window.request_animation_frame(callback.as_ref().unchecked_ref());
            }
        }));
        if let Some(callback) = frame.borrow().as_ref() {
            let _ = window.request_animation_frame(callback.as_ref().unchecked_ref());
        }

        let resize_bridge = bridge.clone();
        let resize_window = window.clone();
        let resize_canvas = canvas.clone();
        let on_resize = Closure::<dyn FnMut()>::new(move || {
            let dpr = resize_window.device_pixel_ratio() as f32;
            let rect = resize_canvas.get_bounding_client_rect();
            resize_bridge.send(&ClientMessage::Resize {
                width: rect.width() as f32 * dpr,
                height: rect.height() as f32 * dpr,
            });
        });
        let observer = ResizeObserver::new(on_resize.as_ref().unchecked_ref())
            .expect("failed to create resize observer");
        observer.observe(&canvas);
        on_resize.forget();
        std::mem::forget(observer);
    });

    let down_drag = drag.clone();
    let on_pointerdown = move |event: PointerEvent| {
        let mut state = down_drag.borrow_mut();
        state.dragging = true;
        state.moved = 0.0;
        state.last_x = event.client_x() as f32;
        state.last_y = event.client_y() as f32;
        drop(state);
        if let Some(canvas) = canvas_ref.get() {
            let _ = canvas.set_pointer_capture(event.pointer_id());
        }
        grabbing.set(true);
    };

    let move_drag = drag.clone();
    let on_pointermove = move |event: PointerEvent| {
        let mut state = move_drag.borrow_mut();
        if !state.dragging {
            return;
        }
        let x = event.client_x() as f32;
        let y = event.client_y() as f32;
        let delta_x = x - state.last_x;
        let delta_y = y - state.last_y;
        state.last_x = x;
        state.last_y = y;
        state.moved += delta_x.abs() + delta_y.abs();
        state.pending_yaw += delta_x;
        state.pending_pitch += delta_y;
    };

    let cancel_drag = drag.clone();
    let on_pointercancel = move |event: PointerEvent| {
        cancel_drag.borrow_mut().dragging = false;
        grabbing.set(false);
        if let Some(canvas) = canvas_ref.get() {
            let _ = canvas.release_pointer_capture(event.pointer_id());
        }
    };

    let up_drag = drag.clone();
    let up_slot = bridge_slot.clone();
    let on_pointerup = move |event: PointerEvent| {
        let was_click = {
            let mut state = up_drag.borrow_mut();
            let clicked = state.dragging && state.moved < 4.0;
            state.dragging = false;
            clicked
        };
        grabbing.set(false);
        if let Some(canvas) = canvas_ref.get() {
            let _ = canvas.release_pointer_capture(event.pointer_id());
            if was_click {
                let rect = canvas.get_bounding_client_rect();
                let ndc_x = ((event.client_x() as f64 - rect.left()) / rect.width()) * 2.0 - 1.0;
                let ndc_y = -(((event.client_y() as f64 - rect.top()) / rect.height()) * 2.0 - 1.0);
                if let Some(bridge) = up_slot.borrow().as_ref() {
                    bridge.send(&ClientMessage::Pick {
                        x: ndc_x as f32,
                        y: ndc_y as f32,
                    });
                }
            }
        }
    };

    let wheel_drag = drag.clone();
    let on_wheel = move |event: WheelEvent| {
        event.prevent_default();
        wheel_drag.borrow_mut().pending_zoom += event.delta_y() as f32;
    };

    let speed_slot = bridge_slot.clone();
    let on_speed = move |event: Event| {
        let value = input_value(&event).parse::<f32>().unwrap_or(1.0);
        speed.set(value);
        if let Some(bridge) = speed_slot.borrow().as_ref() {
            bridge.send(&ClientMessage::SetSpeed { speed: value });
        }
    };

    let color_slot = bridge_slot.clone();
    let on_color = move |event: Event| {
        let (red, green, blue) = hex_to_rgb(&input_value(&event));
        if let Some(bridge) = color_slot.borrow().as_ref() {
            bridge.send(&ClientMessage::SetColor { red, green, blue });
        }
    };

    let jam_slot = bridge_slot.clone();
    let on_jam = move |_| {
        let Some(bridge) = jam_slot.borrow().as_ref().cloned() else {
            return;
        };
        spawn_local(async move {
            let before = bridge.request_stats().await;
            jam_result.set(String::new());
            jammed.set(true);

            let performance = web_sys::window().unwrap().performance().unwrap();
            let start = performance.now();
            while performance.now() - start < 3000.0 {}
            let blocked = performance.now() - start;

            let after = bridge.request_stats().await;
            jammed.set(false);
            let advanced = (after.frames - before.frames) as i64;
            jam_result.set(format!(
                "Main thread blocked {} ms. wgpu advanced {} frames meanwhile.",
                blocked.round() as i64,
                format_thousands(advanced)
            ));
        });
    };

    let canvas_class = move || {
        let cursor = if grabbing.get() {
            "cursor-grabbing"
        } else {
            "cursor-grab"
        };
        format!("block w-full h-full touch-none {cursor}")
    };

    let main_stat_class = move || {
        let base = "px-2.5 py-2 rounded-md border";
        if jammed.get() {
            format!("{base} bg-[#2a1414] border-[#5c1f1f]")
        } else {
            format!("{base} bg-[#161a26] border-[#262b3c]")
        }
    };
    let heartbeat_value_class = move || {
        if jammed.get() {
            "text-[20px] tabular-nums text-[#ff8a8a]"
        } else {
            "text-[20px] tabular-nums"
        }
    };

    view! {
        <div class="fixed inset-0">
            <canvas
                id="canvas"
                node_ref=canvas_ref
                class=canvas_class
                on:pointerdown=on_pointerdown
                on:pointermove=on_pointermove
                on:pointerup=on_pointerup
                on:pointercancel=on_pointercancel
                on:wheel=on_wheel
            ></canvas>
        </div>

        <div class="fixed top-4 left-4 w-[340px] px-[18px] py-4 bg-[#0e1018]/80 border border-[#2a2e3f] rounded-[10px] backdrop-blur-sm text-[13px] leading-[1.5]">
            <h1 class="text-[15px] m-0 mb-1.5">"WebGPU, off the main thread"</h1>
            <p class="m-0 mb-3 text-[#9aa0b4]">
                "The cube, GPU pipeline, and render loop run inside a web worker. The page only forwards events: drag to orbit, scroll to zoom, click to mark a spot. Jam the main thread and watch the cube keep spinning."
            </p>

            <div class="mb-3.5 px-2.5 py-2 rounded-md bg-[#14301f] border border-[#1f5c39] text-[#7ee0a3] break-all">
                "wgpu executes in: "<span>{move || context.get()}</span><br/>
                "renderer: "<span>{move || adapter.get()}</span>
            </div>

            <div class="grid grid-cols-2 gap-2 mb-3.5">
                <div class="px-2.5 py-2 rounded-md bg-[#161a26] border border-[#262b3c]">
                    <div class="text-[#8a90a6] text-[11px] uppercase tracking-[0.04em]">"Worker (wgpu)"</div>
                    <div class="text-[20px] tabular-nums">
                        <span>{move || format!("{:.0}", fps.get())}</span>" fps"
                    </div>
                    <div class="text-[#8a90a6] text-[11px] uppercase tracking-[0.04em]">
                        <span>{move || format_thousands(frames.get() as i64)}</span>" frames"
                    </div>
                </div>
                <div class=main_stat_class>
                    <div class="text-[#8a90a6] text-[11px] uppercase tracking-[0.04em]">"Main thread"</div>
                    <div class=heartbeat_value_class>{move || heartbeat.get().to_string()}</div>
                    <div class="text-[#8a90a6] text-[11px] uppercase tracking-[0.04em]">"heartbeat ticks"</div>
                </div>
            </div>

            <label class="block mb-3">
                <div class="flex justify-between text-[#9aa0b4] mb-1">
                    <span>"Rotation speed"</span>
                    <span>{move || format!("{:.1}", speed.get())}</span>
                </div>
                <input
                    type="range"
                    min="0"
                    max="5"
                    step="0.1"
                    prop:value=move || speed.get().to_string()
                    class="w-full"
                    on:input=on_speed
                />
            </label>

            <label class="block mb-3">
                <div class="flex justify-between text-[#9aa0b4] mb-1"><span>"Cube color"</span></div>
                <input
                    type="color"
                    value="#4d80e6"
                    class="w-full h-[30px] bg-transparent border border-[#262b3c] rounded-md"
                    on:input=on_color
                />
            </label>

            <button
                class="w-full p-2.5 text-white bg-[#b4452f] hover:bg-[#c84e36] border-0 rounded-md cursor-pointer"
                on:click=on_jam
            >
                "Jam main thread for 3 s"
            </button>
            <div class="mt-2.5 min-h-[34px] text-[#cfd3e0] text-[12px]">{move || jam_result.get()}</div>
            <div class="text-[#9aa0b4] text-[12px]">
                "Picked: "<span class="text-[#cfd3e0]">{move || pick_text.get()}</span>
            </div>
        </div>
    }
}

fn input_value(event: &Event) -> String {
    event
        .target()
        .and_then(|target| target.dyn_into::<HtmlInputElement>().ok())
        .map(|input| input.value())
        .unwrap_or_default()
}

fn hex_to_rgb(hex: &str) -> (f32, f32, f32) {
    let parse = |range: std::ops::Range<usize>| {
        hex.get(range)
            .and_then(|slice| u8::from_str_radix(slice, 16).ok())
            .map(|value| value as f32 / 255.0)
            .unwrap_or(0.0)
    };
    (parse(1..3), parse(3..5), parse(5..7))
}

fn format_thousands(value: i64) -> String {
    let negative = value < 0;
    let digits = value.unsigned_abs().to_string();
    let bytes = digits.as_bytes();
    let mut grouped = String::new();
    for (index, byte) in bytes.iter().enumerate() {
        if index > 0 && (bytes.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(*byte as char);
    }
    if negative {
        format!("-{grouped}")
    } else {
        grouped
    }
}
