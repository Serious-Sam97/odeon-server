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
    /// Alguma coisa aconteceu na locadora (R19).
    ///
    /// **Sem escopo desde a R28.** O evento carregava o `circulo_id` de quem
    /// mexeu, e ninguém filtrava por ele — nem o servidor ao publicar, nem a tela
    /// ao receber. Com uma loja só ele deixou de ter até o que significar: o que
    /// acontece na loja acontece pra todo mundo que está nela.
    ///
    /// **Um evento pra quatro ações**, e não quatro variantes: quem ouve faz a
    /// mesma coisa com todas — recarrega a prateleira. O que muda é só a frase
    /// que aparece na tela. Quatro variantes seriam quatro `match` do lado do
    /// cliente pra chegar no mesmo `fetch`.
    ///
    /// É o pedido de volta que justifica isto existir: um bloqueio vira porta
    /// quando quem está com a fita **sabe na hora** que pediram. O barramento
    /// do M3 já entrega isso desde sempre.
    Locadora {
        /// `pegou` | `devolveu` | `pediu` | `venceu`.
        acao: String,
        /// Ausente em `venceu`: a varredura pode devolver várias de uma vez, e
        /// nomear uma delas seria escolher arbitrariamente qual contar.
        caixa_id: Option<Uuid>,
        titulo: Option<String>,
        quem_nome: Option<String>,
    },
    /// Chegou mensagem (R33).
    ///
    /// **Só o aviso, não o texto.** Quem recebe recarrega a conversa — e o
    /// barramento é aberto a todos os aparelhos autenticados, então mandar o
    /// conteúdo aqui entregaria a conversa a quem não é dela. O `para` é o
    /// filtro que cada cliente aplica, como o `user_id` do `ProgrammeStarting`.
    Mensagem {
        de: Uuid,
        de_nome: String,
        para: Uuid,
    },
    /// Um programa que alguém pediu pra ser avisado está começando.
    ///
    /// Vai pelo mesmo barramento do resto: o navegador já mantém UM `EventSource`
    /// aberto, e abrir um segundo canal só pra isto seria conexão a mais pra
    /// dizer a mesma coisa.
    ProgrammeStarting {
        programme_id: i64,
        channel_id: Uuid,
        channel_name: String,
        title: String,
        starts_at: chrono::DateTime<chrono::Utc>,
        /// Pra quem é. Cada aparelho descarta o que não é do usuário logado.
        user_id: Uuid,
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
