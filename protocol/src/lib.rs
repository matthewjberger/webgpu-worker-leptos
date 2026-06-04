use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct CanvasSize {
    pub width: f32,
    pub height: f32,
}

#[derive(Clone, Copy, Serialize, Deserialize)]
pub struct Stats {
    pub frames: f64,
    pub fps: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AdapterInfo {
    pub adapter: String,
    pub backend: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PickResult {
    pub face: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    Init { width: f32, height: f32 },
    Resize { width: f32, height: f32 },
    SetSpeed { speed: f32 },
    SetColor { red: f32, green: f32, blue: f32 },
    Orbit { yaw: f32, pitch: f32 },
    Zoom { amount: f32 },
    Pick { x: f32, y: f32 },
    StatsRequest { id: u32 },
}

#[derive(Clone, Serialize, Deserialize)]
pub enum WorkerMessage {
    Ready { info: AdapterInfo, context: String },
    Stats { stats: Stats },
    StatsReply { id: u32, stats: Stats },
    Picked { hit: Option<PickResult> },
}
