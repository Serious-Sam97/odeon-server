//! R64 — o formato, separado da identificação.
//!
//! ## O que foi medido
//!
//! 18/08/2026, contadores agrupados de `/api/library`:
//!
//! ```text
//! format:série  120
//! format:filme  834
//! format:anime    3
//! ──────────────────
//! soma          957      de 8.333 entradas
//! ```
//!
//! **~7.376 entradas sem etiqueta de formato.** Elas somem de qualquer
//! prateleira e só existem em "tudo".
//!
//! ## A causa é uma dependência que não precisava existir
//!
//! O `format:` só era escrito no `apply_candidate`, junto dos gêneros e do
//! elenco — ou seja, **depois que o provider disse que filme é**. Mas as duas
//! perguntas são diferentes, e o cliente disse isso melhor do que este comentário
//! diria:
//!
//! > *"dá pra saber que algo é um filme sem saber **que** filme é."*
//!
//! O scanner já responde a primeira: `work.kind` sai do `default_kind` da
//! biblioteca e do palpite do nome do arquivo, sem tocar em rede. Quem tem
//! `season_number` é episódio; o resto, com duração de longa, é filme. O dado
//! estava a uma coluna de distância.
//!
//! Medido por `kind`, o que dá pra recuperar:
//!
//! | kind | sem `format:` | vira |
//! |---|---|---|
//! | `episode` | 5.457 | `série` |
//! | `movie` | 147 | `filme` |
//! | `music_video` | 24 | `clipe` |
//! | `other` | 2.182 | **nada** |
//!
//! ## Por que `other` fica de fora
//!
//! `other` é o `kind` que o scanner usa quando **ele mesmo** não sabe — é o
//! "não identificado" do nível de baixo. Escrever `filme` ali seria trocar uma
//! ausência honesta por um palpite com cara de dado, que é o §18 ao contrário.
//! São 2.182 entradas que continuam só em "tudo", e continuam certas.
//!
//! ## O que a identificação ainda decide
//!
//! `anime`. Ele não sai do arquivo — sai de o provider ser o AniList —, e por
//! isso o `apply_candidate` continua mandando aqui. A diferença é que agora ele
//! **substitui** em vez de acrescentar: sem isso um episódio de anime ficaria
//! com `format:série` do scanner e `format:anime` da identificação ao mesmo
//! tempo, e apareceria nas duas prateleiras.

use sqlx::PgPool;
use uuid::Uuid;

/// O formato que dá pra afirmar só olhando o `kind` da obra.
///
/// `None` é resposta, não falha: quer dizer "o scanner também não sabe".
pub fn do_kind(kind: &str) -> Option<&'static str> {
    match kind {
        "episode" => Some("série"),
        "movie" => Some("filme"),
        "music_video" => Some("clipe"),
        // R75 — vídeo de canal. `other` queria dizer "o scanner não sabe"; numa
        // biblioteca com `provider_hint = 'none'` ele sabe, e essa era a última
        // mentira que o modelo contava sobre os 2.511 do YouTube.
        "video" => Some("vídeo"),
        _ => None,
    }
}

/// Grava o formato da obra, **trocando** o que estiver lá.
///
/// A troca é o ponto. `attach_tag` acrescenta, e é o certo pra gênero e país —
/// uma obra tem vários. Formato é um só: uma obra não é filme e série ao mesmo
/// tempo, e deixar os dois faria a mesma entrada aparecer em duas prateleiras
/// que a tela apresenta como excludentes.
pub async fn gravar(pool: &PgPool, work_id: Uuid, formato: &str) -> anyhow::Result<()> {
    sqlx::query(
        "DELETE FROM work_tag wt
          USING tag t
          WHERE t.id = wt.tag_id
            AND wt.work_id = $1
            AND t.namespace = 'format'
            AND t.value <> $2",
    )
    .bind(work_id)
    .bind(formato)
    .execute(pool)
    .await?;

    crate::metadata::attach_tag(pool, work_id, "format", formato).await
}

/// O mesmo, a partir do `kind` — e silencioso quando não há o que afirmar.
pub async fn gravar_do_kind(pool: &PgPool, work_id: Uuid, kind: &str) {
    let Some(formato) = do_kind(kind) else { return };
    if let Err(e) = gravar(pool, work_id, formato).await {
        tracing::warn!(error = %e, kind, "formato do scanner não gravou");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o_kind_do_scanner_ja_diz_o_formato() {
        assert_eq!(do_kind("episode"), Some("série"));
        assert_eq!(do_kind("movie"), Some("filme"));
        assert_eq!(do_kind("music_video"), Some("clipe"));
        assert_eq!(do_kind("video"), Some("vídeo"));
    }

    /// `other` é o "não sei" do scanner. Um palpite aqui seria inventar
    /// metadado — e são 2.182 entradas, o suficiente pra sujar toda prateleira.
    #[test]
    fn other_nao_vira_palpite() {
        assert_eq!(do_kind("other"), None);
        assert_eq!(do_kind("qualquer_coisa"), None);
    }

    /// `anime` não sai daqui de propósito: ele depende de **qual provider**
    /// respondeu, não do arquivo. Quem o escreve é o `apply_candidate`.
    #[test]
    fn anime_nao_sai_do_kind() {
        assert!(!do_kind("episode").is_some_and(|f| f == "anime"));
    }
}
