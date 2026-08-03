//! R22 — a ficha de produção: de onde o filme vem, e em que língua ele fala.
//!
//! O `IDEIAS.md` §0 mediu que o Odeon não busca país, idioma, empresa,
//! orçamento nem bilheteria — e concluiu que o eixo de região da wiki (§30)
//! dependia de acrescentar essas colunas. Medido agora em 40 filmes sorteados,
//! antes de escrever:
//!
//! | | |
//! |---|---|
//! | país de produção | **100%** |
//! | idioma original | **100%** |
//! | empresa produtora | 100% |
//! | orçamento e bilheteria | 92% |
//!
//! **Cobertura não é o que decide o que entra.** A distribuição é:
//!
//! ```text
//! países : US 34 · GB 9 · JP 3 · FR 2 · IT 1 · KR 1 · CA 1 · DE 1
//! idiomas: en 37 · ja 2 · ko 1
//! empresas distintas: 34 — em 40 filmes
//! ```
//!
//! Três leituras, e as três mudam o que este módulo faz:
//!
//! **1. Empresa produtora não entra.** Quase uma por filme. Um eixo em que
//! cada item tem uma obra não é eixo, é lista — a mesma reprovação que o corte
//! de "2+ obras" do §8h aplica às pessoas, e a mesma razão pela qual a R18
//! (§30) recusou o eixo de produção.
//!
//! **2. Orçamento e bilheteria não entram**, apesar dos 92%. O §33 já os traz
//! do Wikidata **com a moeda**, e lá foi decisão explícita que valor em moeda
//! que não sabemos nomear não vira curiosidade. Os campos do TMDB são número
//! puro, sem moeda — pôr "US$" num orçamento em euros é a mentira com cara de
//! metadado que o §18 proíbe. Duas fontes pro mesmo fato, e a segunda pior, é
//! pior que uma fonte só.
//!
//! **3. País e idioma entram, e viram tags** — não colunas. O `tag`/`work_tag`
//! do M2 já é exatamente isto, o filtro por tag de `/api/works` existe desde
//! então, e o guia já resolve gênero e década por ele. Uma coluna exigiria um
//! caminho de consulta novo pra responder a pergunta que `genre:Terror` já
//! responde.
//!
//! E há uma ressalva honesta que só a distribuição mostra, registrada aqui pra
//! quem for desenhar a tela: **este acervo é 85% americano e 92% anglófono.**
//! Um eixo de região aqui tem uma gaveta grande e sete pequenas. Isso não o
//! invalida — "os 6 filmes japoneses que eu tenho" é uma pergunta que ninguém
//! consegue fazer hoje — mas quem montar a tela deve mostrar o **resto**, não
//! o topo, senão o eixo inteiro dirá "Estados Unidos".

use sqlx::PgPool;
use uuid::Uuid;

use super::{regiao, tmdb::Tmdb, Providers};
use crate::jobs::Job;

/// Namespaces das tags. `genre` e `format` já existiam; estes seguem a mesma
/// convenção — chave em inglês, valor em português, como o acervo já faz.
pub const NS_PAIS: &str = "country";
pub const NS_IDIOMA: &str = "lang";

/// De quantos em quantos filmes o job respira: reporta progresso, confere
/// cancelamento e espera.
///
/// As requisições são **sequenciais** de propósito. O §33 fixou a postura
/// deste projeto com serviço alheio ao esperar 1,1 s entre lotes do Wikidata, e
/// aqui o custo total já é pequeno: 548 chamadas de ~0,2 s dão pouco mais de
/// dois minutos. Paralelizar economizaria um minuto e transformaria um job
/// educado num raspador.
const LOTE: usize = 8;
const INTERVALO: std::time::Duration = std::time::Duration::from_millis(400);

/// Aplica país e idioma a uma obra, como tags.
///
/// `Ok(false)` quando o provider não devolveu nada de útil — que é diferente
/// de erro, e é o que permite ao aquecimento contar "sem ficha" separado de
/// "falhou".
pub async fn aplicar(
    pool: &PgPool,
    tmdb: &Tmdb,
    work_id: Uuid,
    tmdb_id: &str,
) -> anyhow::Result<bool> {
    let ficha = tmdb.producao_do_filme(tmdb_id).await?;
    let mut algo = false;

    for p in &ficha.production_countries {
        // O nome vem do provider **em inglês**, mesmo pedindo `pt-BR`. O
        // `regiao::pais` traduz pelo código e cai no nome do provider quando
        // não conhece o país — o que é melhor que um código cru na tag.
        let nome = regiao::pais(&p.iso_3166_1, &p.name);
        if !nome.trim().is_empty() {
            super::attach_tag(pool, work_id, NS_PAIS, nome.trim()).await?;
            algo = true;
        }
    }

    // `xx` é o código do TMDB para "sem diálogo" — é informação, mas não é
    // idioma, e uma tag `lang:xx` seria um rótulo que ninguém sabe ler.
    if let Some(iso) = ficha.original_language.as_deref().filter(|l| *l != "xx") {
        // **Código desconhecido não vira tag.** Aqui não há nome do provider
        // pra cair, e `lang:sr` como rótulo é pior que a ausência do rótulo —
        // é a mesma regra do §18 aplicada a idioma, e a razão de `regiao::idioma`
        // devolver `Option`.
        if let Some(nome) = regiao::idioma(iso) {
            super::attach_tag(pool, work_id, NS_IDIOMA, &nome).await?;
            algo = true;
        }
    }

    Ok(algo)
}

#[derive(Debug, Default, serde::Serialize)]
pub struct AquecimentoStatus {
    pub running: bool,
    pub total: usize,
    pub feitos: usize,
    pub com_ficha: usize,
    pub sem_nada: usize,
    pub falhas: usize,
    pub atual: Option<String>,
}

/// Busca a ficha de todos os filmes identificados de uma vez.
///
/// **É `job`, e a dívida que ele paga tem nome.** O `IDEIAS.md` §8 registrou
/// que a revisita do TMDB desta fase seriam 548 chamadas e que, *"pela terceira
/// vez, um reparo de minutos vai correr dentro de um request"*. Não corre: o
/// molde é o do §34 — estado no banco, progresso visível, cancelamento no ponto
/// seguro, e retomada pelo `WHERE`.
///
/// **A retomada sai de graça e é exata.** O alvo é "filme identificado que
/// ainda não tem tag de país", então rodar de novo continua de onde parou sem
/// nenhuma coluna de controle. É o mesmo truque do `repair-series` (§21) e do
/// aquecimento de trivia.
pub async fn aquecer(pool: PgPool, providers: Providers, mut job: Option<Job>) {
    let Some(tmdb) = providers.tmdb.clone() else {
        tracing::warn!("sem chave do TMDB — aquecimento de produção não roda");
        if let Some(j) = job.take() {
            j.finish(
                &AquecimentoStatus::default(),
                "failed",
                Some("sem chave do TMDB".to_string()),
            )
            .await;
        }
        return;
    };

    let alvos: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT w.id, w.external_ids->>'tmdb', w.title
         FROM work w
         WHERE w.kind = 'movie'
           AND w.match_state IN ('auto', 'confirmed')
           AND w.external_ids ? 'tmdb'
           AND NOT EXISTS (
               SELECT 1 FROM work_tag wt
               JOIN tag t ON t.id = wt.tag_id
               WHERE wt.work_id = w.id AND t.namespace = $1
           )
         ORDER BY w.title",
    )
    .bind(NS_PAIS)
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let mut s = AquecimentoStatus {
        running: true,
        total: alvos.len(),
        ..Default::default()
    };

    for bloco in alvos.chunks(LOTE) {
        // Cancelamento cooperativo, conferido a cada lote — o ponto seguro do
        // §12: no meio de uma requisição não há o que preservar.
        if matches!(&job, Some(j) if j.cancelled().await) {
            s.running = false;
            if let Some(j) = job.take() {
                j.finish(&s, "cancelled", None).await;
            }
            tracing::info!(feitos = s.feitos, "aquecimento de produção cancelado");
            return;
        }

        s.atual = bloco.first().map(|(_, _, t)| t.clone());

        for (id, tmdb_id, _) in bloco {
            match aplicar(&pool, &tmdb, *id, tmdb_id).await {
                Ok(true) => s.com_ficha += 1,
                Ok(false) => s.sem_nada += 1,
                Err(e) => {
                    // Falha não marca nada no banco, então o filme continua
                    // elegível na próxima passada — que é o que torna isto
                    // retomável de verdade.
                    s.falhas += 1;
                    tracing::warn!(error = %e, "ficha de produção falhou");
                }
            }
        }

        s.feitos += bloco.len();
        if let Some(j) = &job {
            j.tick(&s, s.feitos as i64, Some(s.total as i64)).await;
        }
        tokio::time::sleep(INTERVALO).await;
    }

    s.running = false;
    s.atual = None;
    tracing::info!(
        com_ficha = s.com_ficha,
        sem_nada = s.sem_nada,
        falhas = s.falhas,
        "aquecimento de produção concluído"
    );
    if let Some(j) = job {
        j.finish(&s, "succeeded", None).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Os namespaces seguem a convenção que `genre` e `format` fixaram: chave
    /// em inglês. Trocá-la aqui faria a taxonomia falar duas línguas.
    #[test]
    fn os_namespaces_seguem_a_convencao() {
        assert_eq!(NS_PAIS, "country");
        assert_eq!(NS_IDIOMA, "lang");
        assert!(NS_PAIS.chars().all(|c| c.is_ascii_lowercase()));
        assert!(NS_IDIOMA.chars().all(|c| c.is_ascii_lowercase()));
    }

    /// O valor da tag é o nome em português, como `genre:Ficção científica`.
    /// Um código cru (`US`, `ja`) seria um rótulo que ninguém lê.
    #[test]
    fn o_valor_da_tag_e_nome_e_nao_codigo() {
        assert_eq!(regiao::pais("US", "United States of America"), "Estados Unidos");
        assert_eq!(regiao::idioma("ja").as_deref(), Some("japonês"));
        // País desconhecido cai no nome do provider — melhor que o código.
        assert_eq!(regiao::pais("ZZ", "Ruritânia"), "Ruritânia");
        // Idioma desconhecido não vira tag: aqui não há nome pra cair (§18).
        assert_eq!(regiao::idioma("zz"), None);
    }
}
