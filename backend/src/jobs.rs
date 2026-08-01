//! Registro persistente das operações longas.
//!
//! O que muda em relação ao que havia: nada de comportamento, tudo de memória.
//! As operações continuam sendo `tokio::spawn` com um `Arc<Mutex<Status>>` — o
//! que se ganha é que o BANCO passa a saber que elas existiram.
//!
//! Isso resolve três coisas que doeram na implantação deste servidor:
//!
//!   1. um restart no meio deixava `running: false`, indistinguível de "nunca
//!      rodou". Agora vira `interrupted`, com a razão escrita;
//!   2. não havia como cancelar: parar exigia matar o processo;
//!   3. não havia histórico — nem "quando foi a última varredura".
//!
//! **O cancelamento é cooperativo de propósito.** Quem pede só marca a coluna;
//! o worker decide onde é seguro parar. Interromper no meio de uma transação,
//! ou entre gravar a obra e gravar a coleção dela, deixaria estado pela metade
//! — que é pior do que esperar o item corrente terminar.

use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Um job em andamento, do ponto de vista de quem o executa.
pub struct Job {
    pub id: Uuid,
    pool: PgPool,
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct JobRow {
    pub id: Uuid,
    pub kind: String,
    pub state: String,
    pub progress: serde_json::Value,
    pub total: Option<i32>,
    pub done: i32,
    pub failed: i32,
    pub current: Option<String>,
    pub reasons: serde_json::Value,
    pub error: Option<String>,
    pub cancel_requested: bool,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub finished_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Job {
    /// Abre um job. Devolve `None` se já houver um do mesmo tipo rodando — a
    /// exclusão vem do índice único, não de uma flag em memória que cada módulo
    /// checava por conta própria.
    pub async fn start(
        pool: &PgPool,
        kind: &str,
        params: serde_json::Value,
        requested_by: Option<Uuid>,
    ) -> Option<Self> {
        let id: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO job (kind, params, requested_by) VALUES ($1, $2, $3)
             ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(kind)
        .bind(params)
        .bind(requested_by)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

        id.map(|id| Self {
            id,
            pool: pool.clone(),
        })
    }

    /// Carimba o progresso. O `progress` guarda o Status inteiro em JSON, que é
    /// o que permite os endpoints `/status` continuarem devolvendo o mesmo
    /// formato de sempre — há quatro alvos de cliente lendo aquilo.
    pub async fn tick(&self, progress: &impl Serialize, done: i64, total: Option<i64>) {
        let _ = sqlx::query(
            "UPDATE job SET progress = $2, done = $3, total = COALESCE($4, total),
                            heartbeat_at = now()
             WHERE id = $1",
        )
        .bind(self.id)
        .bind(serde_json::to_value(progress).unwrap_or_default())
        .bind(done as i32)
        .bind(total.map(|t| t as i32))
        .execute(&self.pool)
        .await;
    }

    /// Alguém pediu pra parar? Consultado pelo worker entre itens.
    pub async fn cancelled(&self) -> bool {
        sqlx::query_scalar::<_, bool>("SELECT cancel_requested FROM job WHERE id = $1")
            .bind(self.id)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(false)
    }

    pub async fn finish(self, progress: &impl Serialize, state: &str, error: Option<String>) {
        let _ = sqlx::query(
            "UPDATE job SET state = $2, error = $3, progress = $4, finished_at = now()
             WHERE id = $1",
        )
        .bind(self.id)
        .bind(state)
        .bind(error)
        .bind(serde_json::to_value(progress).unwrap_or_default())
        .execute(&self.pool)
        .await;
    }
}

/// Marca como interrompido o que ficou pendurado de uma execução anterior.
///
/// Roda no boot, ANTES de servir. Sem isto o índice único bloquearia qualquer
/// operação nova: o job morto continuaria "rodando" para sempre, e o servidor
/// recusaria varrer alegando que já há uma varredura em andamento — um deadlock
/// que só um `DELETE` manual resolveria.
pub async fn recover(pool: &PgPool) -> u64 {
    sqlx::query(
        r#"
        UPDATE job SET
            state = 'interrupted',
            finished_at = now(),
            reasons = reasons || '["o processo foi encerrado durante a execução"]'::jsonb
        WHERE state IN ('running', 'queued')
        "#,
    )
    .execute(pool)
    .await
    .map(|r| r.rows_affected())
    .unwrap_or(0)
}

/// O job mais recente de um tipo. É daqui que os `/status` leem.
pub async fn latest(pool: &PgPool, kind: &str) -> Option<JobRow> {
    sqlx::query_as(
        "SELECT id, kind, state, progress, total, done, failed, current, reasons,
                error, cancel_requested, started_at, finished_at
         FROM job WHERE kind = $1 ORDER BY started_at DESC LIMIT 1",
    )
    .bind(kind)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

pub async fn list(pool: &PgPool, limit: i64) -> Vec<JobRow> {
    sqlx::query_as(
        "SELECT id, kind, state, progress, total, done, failed, current, reasons,
                error, cancel_requested, started_at, finished_at
         FROM job ORDER BY started_at DESC LIMIT $1",
    )
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await
    .unwrap_or_default()
}

/// Pede o cancelamento. Não mata nada — marca, e o worker para no próximo
/// ponto seguro.
pub async fn request_cancel(pool: &PgPool, id: Uuid) -> bool {
    sqlx::query(
        "UPDATE job SET cancel_requested = true,
                        reasons = reasons || '[\"cancelamento pedido\"]'::jsonb
         WHERE id = $1 AND state = 'running'",
    )
    .bind(id)
    .execute(pool)
    .await
    .map(|r| r.rows_affected() > 0)
    .unwrap_or(false)
}
