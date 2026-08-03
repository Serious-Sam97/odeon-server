//! A emissora do Odeon: canais que ele mesmo programa, do próprio acervo.
//!
//! A diferença para o resto do `live` é de natureza. Ali o Odeon **sintoniza**
//! — uma fonte publica canais e grade, e daqui pra frente é leitura. Aqui ele
//! **programa**: os canais não existem em lugar nenhum, são uma função da data
//! sobre a biblioteca.
//!
//! Isso tem três consequências que valem mais que o código:
//!
//!  1. **Não há stream.** Sintonizar é tocar o arquivo no offset que o relógio
//!     manda, e o M6 faz isso desde sempre (`?start=`). Nenhum ffmpeg a mais,
//!     nenhuma sessão contínua, nenhum canal "no ar" gastando CPU sem
//!     ninguém assistindo.
//!  2. **Não há tabela.** A grade não é gravada: é recalculada. Duas chamadas
//!     no mesmo dia devolvem exatamente a mesma programação, em qualquer
//!     aparelho, sem nada pra sincronizar nem pra expirar.
//!  3. **Não há daemon.** O `vigiar_grade` do §24 existe porque a grade de
//!     terceiro seca; esta não seca nunca.
//!
//! Medido no acervo em 02/08/2026: 9.018 obras tocáveis, 4.929 horas — 205
//! dias de programação sem repetir um título.

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

/// Um canal da casa. Definido em código de propósito: é editorial, não dado —
/// mudar a linha de um canal é mudar o que o Odeon é, não configurar algo.
pub struct CanalOdeon {
    pub slug: &'static str,
    pub nome: &'static str,
    pub numero: &'static str,
    /// Os rótulos crus do provider. São dois vocabulários no mesmo acervo (o de
    /// filme vem em pt-BR, o de série em inglês) — ver §20.
    pub generos: &'static [&'static str],
}

pub const CANAIS: &[CanalOdeon] = &[
    CanalOdeon {
        slug: "odeon-1",
        nome: "Odeon 1",
        numero: "101",
        generos: &["Ficção científica", "Ação", "Drama", "Aventura"],
    },
    CanalOdeon {
        slug: "odeon-corujao",
        nome: "Odeon Corujão",
        numero: "102",
        generos: &["Terror", "Thriller", "Mistério", "Crime"],
    },
    CanalOdeon {
        slug: "odeon-matine",
        nome: "Odeon Matinê",
        numero: "103",
        generos: &["Família", "Animação", "Comédia"],
    },
];

pub fn canal(slug: &str) -> Option<&'static CanalOdeon> {
    CANAIS.iter().find(|c| c.slug == slug)
}

/// Fuso da grade, em horas inteiras a partir do UTC.
///
/// A grade é ancorada na **meia-noite local**, não na UTC: um canal chamado
/// "Matinê" que começa às 21h da véspera não é uma matinê. Como o Brasil não
/// tem horário de verão desde 2019, um deslocamento fixo resolve sem trazer a
/// `chrono-tz` (e sem a tabela de fusos que ela carrega junto) — se um dia
/// precisar de DST, é aqui que entra.
///
/// **Público porque a locadora também vira à meia-noite local** (§36): a
/// prateleira roda na segunda-feira, e "segunda-feira" tem que querer dizer a
/// mesma coisa que a grade quer dizer por "hoje". Duas leituras de fuso
/// divergindo fariam a loja virar num horário e a emissora noutro.
pub fn deslocamento() -> Duration {
    let h: i64 = std::env::var("ODEON_TZ_OFFSET")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(-3);
    Duration::hours(h.clamp(-12, 14))
}

/// O dia da grade a que um instante pertence, e a meia-noite local dele.
fn dia_e_ancora(t: DateTime<Utc>) -> (NaiveDate, DateTime<Utc>) {
    let local = t + deslocamento();
    let dia = local.date_naive();
    let ancora = Utc.from_utc_datetime(&dia.and_time(NaiveTime::MIN)) - deslocamento();
    (dia, ancora)
}

#[derive(Debug, Serialize)]
pub struct ProgramaOdeon {
    /// `slug:índice` — estável no dia, e é o que o cliente usa como chave.
    pub id: String,
    pub canal: &'static str,
    pub canal_nome: &'static str,
    pub numero: &'static str,
    pub work_id: Uuid,
    pub media_file_id: Option<Uuid>,
    pub title: String,
    pub year: Option<i32>,
    pub arte: Option<String>,
    pub categoria: Option<String>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct Candidata {
    id: Uuid,
    title: String,
    year: Option<i32>,
    runtime_seconds: i32,
    arte: Option<String>,
    categoria: Option<String>,
    media_file_id: Option<Uuid>,
}

/// O intervalo entre programas. Quatro minutos é o que uma emissora usa pra
/// respirar — e sem ele os filmes emendariam em horários quebrados demais pra
/// grade ficar legível.
const INTERVALO_MIN: i64 = 4;

/// A grade de um canal no dia de `instante`, do começo do dia em diante.
///
/// A ordem sai de `md5(dia || slug || id)` **no banco**: determinística, igual
/// para todo cliente, e sem trazer o acervo inteiro para a memória só pra
/// embaralhar.
pub async fn grade(
    pool: &PgPool,
    canal: &'static CanalOdeon,
    instante: DateTime<Utc>,
) -> anyhow::Result<Vec<ProgramaOdeon>> {
    let (dia, ancora) = dia_e_ancora(instante);
    let generos: Vec<String> = canal.generos.iter().map(|g| g.to_string()).collect();
    let semente = format!("{dia}{}", canal.slug);

    let candidatas: Vec<Candidata> = sqlx::query_as(
        r#"
        SELECT w.id, w.title, w.year, w.runtime_seconds,
               COALESCE(w.artwork->>'backdrop', w.artwork->>'poster') AS arte,
               (SELECT t.value FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                 WHERE wt.work_id = w.id AND t.namespace = 'genre'
                   AND t.value = ANY($1) LIMIT 1) AS categoria,
               (SELECT m.id FROM media_file m
                 WHERE m.work_id = w.id AND m.status = 'probed'
                 ORDER BY m.size_bytes DESC LIMIT 1) AS media_file_id
        FROM work w
        WHERE w.kind = 'movie'
          -- 40 min a 3h: curta-metragem não sustenta faixa e épico de 4h come
          -- o dia inteiro de um canal só.
          AND w.runtime_seconds BETWEEN 2400 AND 10800
          AND w.artwork ? 'poster'
          AND EXISTS (SELECT 1 FROM work_tag wt JOIN tag t ON t.id = wt.tag_id
                       WHERE wt.work_id = w.id AND t.namespace = 'genre'
                         AND t.value = ANY($1))
          AND EXISTS (SELECT 1 FROM media_file m
                       WHERE m.work_id = w.id AND m.status = 'probed')
        ORDER BY md5($2 || w.id::text)
        LIMIT 40
        "#,
    )
    .bind(&generos)
    .bind(&semente)
    .fetch_all(pool)
    .await?;

    let mut grade = Vec::new();
    let mut t = ancora;
    let fim_do_dia = ancora + Duration::hours(24);
    for (i, c) in candidatas.iter().enumerate() {
        if t >= fim_do_dia {
            break;
        }
        let fim = t + Duration::seconds(c.runtime_seconds as i64);
        grade.push(ProgramaOdeon {
            id: format!("{}:{i}", canal.slug),
            canal: canal.slug,
            canal_nome: canal.nome,
            numero: canal.numero,
            work_id: c.id,
            media_file_id: c.media_file_id,
            title: c.title.clone(),
            year: c.year,
            arte: c.arte.clone(),
            categoria: c.categoria.clone(),
            starts_at: t,
            ends_at: fim,
        });
        t = fim + Duration::minutes(INTERVALO_MIN);
    }
    Ok(grade)
}

/// A grade de todos os canais da casa, cobrindo a janela pedida.
///
/// Atravessa a virada do dia buscando também o dia seguinte quando a janela
/// passa da meia-noite — senão a linha do tempo terminaria num vazio às 23h59.
pub async fn grade_toda(
    pool: &PgPool,
    agora: DateTime<Utc>,
    ate: DateTime<Utc>,
) -> anyhow::Result<Vec<ProgramaOdeon>> {
    let mut tudo = Vec::new();
    for c in CANAIS {
        let mut do_canal = grade(pool, c, agora).await?;
        if dia_e_ancora(ate).0 != dia_e_ancora(agora).0 {
            do_canal.extend(grade(pool, c, ate).await?);
        }
        tudo.extend(do_canal.into_iter().filter(|p| p.ends_at > agora && p.starts_at < ate));
    }
    tudo.sort_by(|a, b| (a.canal, a.starts_at).cmp(&(b.canal, b.starts_at)));
    Ok(tudo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ancora_e_a_meia_noite_local() {
        // 02/08/2026 02:00 UTC é ainda 01/08 às 23:00 em -3 — o dia da grade
        // tem que ser 01/08, senão a virada do dia acontece na hora errada e
        // o Corujão começa de manhã.
        unsafe { std::env::set_var("ODEON_TZ_OFFSET", "-3") };
        let t = Utc.with_ymd_and_hms(2026, 8, 2, 2, 0, 0).unwrap();
        let (dia, ancora) = dia_e_ancora(t);
        assert_eq!(dia.to_string(), "2026-08-01");
        assert_eq!(ancora.to_rfc3339(), "2026-08-01T03:00:00+00:00");
    }

    #[test]
    fn canal_por_slug() {
        assert!(canal("odeon-corujao").is_some());
        assert!(canal("nao-existe").is_none());
    }
}
