use std::cell::RefCell;
use std::rc::Rc;

use protocol::{ClientMessage, Stats};
use wasm_bindgen::JsValue;
use wasm_bindgen_futures::JsFuture;
use web_sys::{OffscreenCanvas, Worker};

pub type StatsResolver = Rc<RefCell<Option<js_sys::Function>>>;

#[derive(Clone)]
pub struct Bridge {
    worker: Worker,
    pending_stats: StatsResolver,
}

impl Bridge {
    pub fn new(worker: Worker, pending_stats: StatsResolver) -> Self {
        Self {
            worker,
            pending_stats,
        }
    }

    pub fn send(&self, message: &ClientMessage) {
        let envelope = js_sys::Object::new();
        let value = serde_wasm_bindgen::to_value(message).unwrap_or(JsValue::NULL);
        let _ = js_sys::Reflect::set(&envelope, &JsValue::from_str("message"), &value);
        let _ = self.worker.post_message(&envelope);
    }

    pub fn send_init(&self, canvas: OffscreenCanvas, width: f32, height: f32) {
        let envelope = js_sys::Object::new();
        let value = serde_wasm_bindgen::to_value(&ClientMessage::Init { width, height })
            .unwrap_or(JsValue::NULL);
        let _ = js_sys::Reflect::set(&envelope, &JsValue::from_str("message"), &value);
        let _ = js_sys::Reflect::set(&envelope, &JsValue::from_str("canvas"), &canvas);
        let transfer = js_sys::Array::of1(&canvas);
        let _ = self.worker.post_message_with_transfer(&envelope, &transfer);
    }

    pub async fn request_stats(&self) -> Stats {
        let pending_stats = self.pending_stats.clone();
        let promise = js_sys::Promise::new(&mut |resolve, _reject| {
            *pending_stats.borrow_mut() = Some(resolve);
        });
        self.send(&ClientMessage::StatsRequest { id: 0 });
        let value = JsFuture::from(promise).await.unwrap_or(JsValue::NULL);
        serde_wasm_bindgen::from_value(value).unwrap_or(Stats {
            frames: 0.0,
            fps: 0.0,
        })
    }
}
