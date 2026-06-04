use std::cell::RefCell;
use std::rc::Rc;

use protocol::{CanvasSize, ClientMessage, Stats, WorkerMessage};
use renderer::WgpuApp;
use wasm_bindgen::prelude::*;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::spawn_local;
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, OffscreenCanvas};

type AppSlot = Rc<RefCell<Option<WgpuApp>>>;
type FrameLoop = Rc<RefCell<Option<Closure<dyn FnMut()>>>>;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();

    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let app_slot: AppSlot = Rc::new(RefCell::new(None));

    let handler_scope = scope.clone();
    let onmessage = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        handle_message(&handler_scope, &app_slot, event);
    });
    scope.set_onmessage(Some(onmessage.as_ref().unchecked_ref()));
    onmessage.forget();
}

fn handle_message(scope: &DedicatedWorkerGlobalScope, app_slot: &AppSlot, event: MessageEvent) {
    let data = event.data();
    let Ok(payload) = js_sys::Reflect::get(&data, &JsValue::from_str("message")) else {
        return;
    };
    let Ok(message) = serde_wasm_bindgen::from_value::<ClientMessage>(payload) else {
        return;
    };

    match message {
        ClientMessage::Init { width, height } => {
            let Some(canvas) = canvas_from(&data) else {
                return;
            };
            let scope = scope.clone();
            let app_slot = app_slot.clone();
            spawn_local(async move {
                let app = WgpuApp::create(canvas, CanvasSize { width, height }).await;
                post(
                    &scope,
                    &WorkerMessage::Ready {
                        info: app.adapter_info(),
                        context: app.context(),
                    },
                );
                *app_slot.borrow_mut() = Some(app);
                start_render_loop(scope, app_slot);
            });
        }
        ClientMessage::Resize { width, height } => {
            if let Some(app) = app_slot.borrow_mut().as_mut() {
                app.resize(CanvasSize { width, height });
            }
        }
        ClientMessage::SetSpeed { speed } => {
            if let Some(app) = app_slot.borrow_mut().as_mut() {
                app.set_speed(speed);
            }
        }
        ClientMessage::SetColor { red, green, blue } => {
            if let Some(app) = app_slot.borrow_mut().as_mut() {
                app.set_color(red, green, blue);
            }
        }
        ClientMessage::Orbit { yaw, pitch } => {
            if let Some(app) = app_slot.borrow_mut().as_mut() {
                app.orbit(yaw, pitch);
            }
        }
        ClientMessage::Zoom { amount } => {
            if let Some(app) = app_slot.borrow_mut().as_mut() {
                app.zoom(amount);
            }
        }
        ClientMessage::Pick { x, y } => {
            let hit = app_slot
                .borrow_mut()
                .as_mut()
                .and_then(|app| app.pick(x, y));
            post(scope, &WorkerMessage::Picked { hit });
        }
        ClientMessage::StatsRequest { id } => {
            let stats = app_slot
                .borrow()
                .as_ref()
                .map(|app| app.stats())
                .unwrap_or(Stats {
                    frames: 0.0,
                    fps: 0.0,
                });
            post(scope, &WorkerMessage::StatsReply { id, stats });
        }
    }
}

fn start_render_loop(scope: DedicatedWorkerGlobalScope, app_slot: AppSlot) {
    let frame: FrameLoop = Rc::new(RefCell::new(None));
    let frame_handle = frame.clone();
    let loop_scope = scope.clone();
    let last_push = Rc::new(RefCell::new(0.0_f64));

    *frame.borrow_mut() = Some(Closure::<dyn FnMut()>::new(move || {
        if let Some(app) = app_slot.borrow_mut().as_mut() {
            app.update();
            if let Some(performance) = loop_scope.performance() {
                let now = performance.now();
                let mut last = last_push.borrow_mut();
                if now - *last > 250.0 {
                    *last = now;
                    post(&loop_scope, &WorkerMessage::Stats { stats: app.stats() });
                }
            }
        }
        if let Some(callback) = frame_handle.borrow().as_ref() {
            let _ = loop_scope.request_animation_frame(callback.as_ref().unchecked_ref());
        }
    }));

    if let Some(callback) = frame.borrow().as_ref() {
        let _ = scope.request_animation_frame(callback.as_ref().unchecked_ref());
    }
}

fn canvas_from(data: &JsValue) -> Option<OffscreenCanvas> {
    js_sys::Reflect::get(data, &JsValue::from_str("canvas"))
        .ok()
        .and_then(|value| value.dyn_into::<OffscreenCanvas>().ok())
}

fn post(scope: &DedicatedWorkerGlobalScope, message: &WorkerMessage) {
    if let Ok(value) = serde_wasm_bindgen::to_value(message) {
        let _ = scope.post_message(&value);
    }
}
