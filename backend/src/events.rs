//! Barramento de eventos + SSE.
//!
//! É isto que faz "sync entre devices" ser sync de verdade e não só "retoma de
//! onde parou quando você abre". Pausar no notebook aparece na TV na hora.
//!
//! SSE e não WebSocket: o tráfego é unidirecional (servidor → clientes), o
//! browser reconecta sozinho, e passa por qualquer proxy. WebSocket seria
//! complexidade sem contrapartida.

use std::convert::Infallible;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use serde::Serialize;
use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::{Stream, StreamExt};
use uuid::Uuid;

use crate::AppState;

/// Capacidade do canal. Se um cliente lento ficar pra trás além disso ele perde
/// eventos — e tudo bem: o estado real está no banco, o evento é só um aviso.
const CHANNEL_CAPACITY: usize = 256;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppEvent {
    Progress {
        work_id: Uuid,
        position_seconds: f64,
        duration_seconds: Option<f64>,
        finished: bool,
        /// Quem emitiu. O próprio device ignora o próprio eco.
        device_id: String,
    },
    ScanFinished {
        added: u64,
        updated: u64,
    },
    MatchFinished {
        auto: u64,
        needs_review: u64,
    },
    ScrubFinished {
        done: u64,
        failed: u64,
    },
}

pub type Bus = broadcast::Sender<AppEvent>;

pub fn bus() -> Bus {
    broadcast::channel(CHANNEL_CAPACITY).0
}

/// Publica sem se importar se há alguém ouvindo — `send` só falha quando não há
/// nenhum assinante, o que é normal.
pub fn publish(bus: &Bus, event: AppEvent) {
    let _ = bus.send(event);
}

pub async fn stream(
    State(state): State<AppState>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let receiver = state.events.subscribe();

    let stream = BroadcastStream::new(receiver).filter_map(|message| match message {
        Ok(event) => Event::default().json_data(event).ok().map(Ok),
        // cliente ficou pra trás: pula o que perdeu em vez de derrubar a conexão
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}
