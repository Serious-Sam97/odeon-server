//! R24 — a retrospectiva.
//!
//! ## A regra que decidiu a tela, e ela não é sobre design
//!
//! O `IDEIAS.md` §6.2 aprovou retrospectiva **e** placar, separados, e deixou
//! escrito por que a retrospectiva passa nos pilares e o placar não:
//!
//! > O §8f já sabe dizer *"você costuma terminar palestra (100%)"* — isso é
//! > conquista de verdade, porque **descreve quem você é** em vez de te dar
//! > ponto.
//!
//! Então esta tela não é um painel de números: é o perfil de gosto do M5 com
//! roupa nova. Nada aqui é declarado, nada é pontuado, e nada premia volume.
//!
//! ## E a regra que decidiu o que NÃO aparece
//!
//! Medido antes de escrever, no acervo real:
//!
//! | | |
//! |---|---|
//! | eventos | 128 |
//! | obras tocadas | 18 — **2 terminadas, 12 largadas** |
//! | pessoas com afinidade | 15 |
//! | **empréstimos** | **0** |
//! | **avaliações** | **0** |
//! | dias com atividade | 3 |
//!
//! A §7 do `IDEIAS.md` supunha que a esta altura haveria *"aluguéis,
//! devoluções, atrasos, notas"* pra contar. **Não há.** A R19 e a R23
//! construíram o motor; ninguém rodou ele ainda.
//!
//! Isso é exatamente a armadilha que a R15 (§26) documentou — a tela que não
//! estava crua, estava *"confiante sobre o que não sabia"*. A defesa aqui é
//! estrutural, não editorial: **cada bloco só existe quando tem o que dizer.**
//! Uma retrospectiva que anuncia "0 empréstimos · 0 devoluções atrasadas · 0
//! notas" ensina a não ler a tela, e aí o dia em que houver algo também não
//! será lido. É o §24 aplicado ao que deveria ser a tela mais pessoal do
//! produto.
//!
//! Hoje ela rende quatro blocos. Quando a locadora rodar, rende seis. Nenhum
//! deles precisa mudar pra isso acontecer.

use axum::extract::State;
use axum::Json;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::curation::taste;
use crate::error::AppResult;
use crate::AppState;

/// Um bloco da retrospectiva.
///
/// `titulo` é o rótulo; `frase` é a afirmação; `detalhe` são os itens que a
/// sustentam. **A frase é montada no servidor**, como os `reasons` do score do
/// §8b e as curiosidades do §32 — montá-la no cliente seria uma segunda
/// gramática pra manter.
#[derive(Debug, Serialize)]
pub struct Bloco {
    pub chave: &'static str,
    pub titulo: &'static str,
    pub frase: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub detalhe: Vec<Item>,
}

#[derive(Debug, Serialize)]
pub struct Item {
    pub rotulo: String,
    pub nota: Option<String>,
    /// Pôster, quando o item é uma obra; retrato, quando é pessoa.
    pub imagem: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Retrospectiva {
    pub blocos: Vec<Bloco>,
    /// Quantos blocos **não** apareceram por falta de material.
    ///
    /// Não é enfeite: é a tela dizendo por que ela é curta, em vez de deixar a
    /// pessoa concluir que o Odeon não sabe nada dela. A R15 (§26) errou por
    /// não ter isto.
    pub calados: usize,
    pub desde: Option<chrono::DateTime<chrono::Utc>>,
}

/// Quantas pessoas e tags cabem num bloco antes dele virar tabela.
const NO_BLOCO: usize = 6;

pub async fn retrospectiva(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Retrospectiva>> {
    let perfil = taste::build(&state.pool, user.id)
        .await
        .map_err(crate::error::AppError::Other)?;

    let mut blocos: Vec<Bloco> = Vec::new();
    let mut calados = 0usize;

    // --- 1. o que você faz com o que abre -------------------------------
    //
    // O sinal mais honesto do projeto, e o que abre a retrospectiva: **largar
    // conta tanto quanto terminar.** Um painel de gamificação esconderia as
    // 12 largadas porque elas "não pontuam"; aqui elas são metade do retrato,
    // e a mais interessante — quem larga 12 de 18 é uma pessoa que experimenta.
    if perfil.works_touched > 0 {
        let t = perfil.finished;
        let l = perfil.abandoned;
        let frase = if l > t * 2 {
            format!(
                "Você abriu {} e terminou {}. Larga bem mais do que termina — \
                 é gente que experimenta, não que coleciona.",
                obras(perfil.works_touched),
                t
            )
        } else if t > 0 {
            format!(
                "Você abriu {} e terminou {}. Quando começa, costuma ir até o fim.",
                obras(perfil.works_touched),
                t
            )
        } else {
            format!(
                "Você abriu {} e ainda não terminou nenhuma — está na fase de \
                 provar as coisas.",
                obras(perfil.works_touched)
            )
        };
        blocos.push(Bloco { chave: "habito", titulo: "O que você faz", frase, detalhe: vec![] });
    } else {
        calados += 1;
    }

    // --- 2. quem você não larga -----------------------------------------
    //
    // `person_affinity` já exige 2+ obras (§8h): com uma só, o elenco inteiro
    // de um filme viraria gosto favorito.
    //
    // **E só afinidade positiva**, que é o que o título promete. A primeira
    // versão dizia "15 pessoas aparecem mais de uma vez no que você termina" e
    // a lista vinha de `person_affinity` inteira — que conta obras **abertas**,
    // não terminadas, e inclui quem você largou. A frase afirmava o que o dado
    // não sustentava: §18 numa tela em vez de num metadado.
    let queridas: Vec<&crate::curation::taste::PersonAffinity> = perfil
        .person_affinity
        .iter()
        .filter(|p| p.score > 0.0)
        .collect();
    if !queridas.is_empty() {
        let pessoas: Vec<Uuid> = queridas.iter().take(NO_BLOCO).map(|p| p.id).collect();
        let retratos: Vec<(Uuid, Option<String>)> =
            sqlx::query_as("SELECT id, image_path FROM person WHERE id = ANY($1)")
                .bind(&pessoas)
                .fetch_all(&state.pool)
                .await?;

        let detalhe = queridas
            .iter()
            .take(NO_BLOCO)
            .map(|p| Item {
                rotulo: p.name.clone(),
                nota: Some(format!("{} obras", p.works)),
                imagem: retratos
                    .iter()
                    .find(|(id, _)| *id == p.id)
                    .and_then(|(_, img)| img.clone()),
            })
            .collect();

        blocos.push(Bloco {
            chave: "pessoas",
            titulo: "Quem você não larga",
            frase: format!(
                "{} {} em mais de uma obra que você gostou.",
                queridas.len(),
                if queridas.len() == 1 { "pessoa aparece" } else { "pessoas aparecem" }
            ),
            detalhe,
        });
    } else {
        calados += 1;
    }

    // --- 3. o que você procura ------------------------------------------
    //
    // Só afinidade positiva. As negativas existem e são informação boa pra
    // curadoria, mas "você odeia documentário" é uma frase que ninguém pediu
    // pra ler sobre si — e a retrospectiva descreve, não julga.
    let gostos: Vec<&(String, f32)> = perfil
        .tag_affinity
        .iter()
        .filter(|(_, v)| *v > 0.15)
        .take(NO_BLOCO)
        .collect();
    if !gostos.is_empty() {
        blocos.push(Bloco {
            chave: "gostos",
            titulo: "O que você procura",
            frase: "Sai do que você termina, não do que você diz.".into(),
            detalhe: gostos
                .iter()
                .map(|(tag, _)| Item {
                    // `genre:Crime` vira "Crime": o namespace é vocabulário de
                    // banco, e a retrospectiva fala português.
                    rotulo: tag.split_once(':').map(|(_, v)| v).unwrap_or(tag).to_string(),
                    nota: None,
                    imagem: None,
                })
                .collect(),
        });
    } else {
        calados += 1;
    }

    // --- 4. a que horas -------------------------------------------------
    if let Some(pico) = pico_do_dia(&perfil.hour_histogram) {
        blocos.push(Bloco {
            chave: "hora",
            titulo: "A que horas",
            frase: format!("Você assiste mais {}.", faixa_do_dia(pico)),
            detalhe: vec![],
        });
    } else {
        calados += 1;
    }

    // --- 5. a locadora (R19) --------------------------------------------
    //
    // Aqui moram os fatos que o §6.2 chamou de "combustível legítimo": quem
    // devolveu atrasado, quem devolveu sem rebobinar. Nenhum deles é métrica
    // inventada — são coisas que aconteceram entre pessoas.
    let loc: Option<(i64, i64, i64, i64)> = sqlx::query_as(
        "SELECT count(*),
                count(*) FILTER (WHERE devolvido_em IS NOT NULL),
                count(*) FILTER (WHERE devolvido_em > vence_em),
                count(*) FILTER (WHERE devolvido_como = 'rebobinada')
         FROM emprestimo WHERE user_id = $1",
    )
    .bind(user.id)
    .fetch_optional(&state.pool)
    .await?;

    match loc {
        Some((pegos, _, atrasos, rebobinadas)) if pegos > 0 => {
            let mut frase = format!("Você pegou {} na locadora.", caixas(pegos));
            if atrasos > 0 {
                frase.push_str(&format!(
                    " {} {} atrasada{}.",
                    if atrasos == pegos { "Todas" } else { "Dessas," },
                    if atrasos == pegos { "voltaram".to_string() } else { format!("{atrasos} voltou") },
                    if atrasos > 1 { "s" } else { "" }
                ));
            }
            if rebobinadas > 0 {
                frase.push_str(&format!(" Você rebobinou {rebobinadas}."));
            }
            blocos.push(Bloco { chave: "locadora", titulo: "Na locadora", frase, detalhe: vec![] });
        }
        _ => calados += 1,
    }

    // --- 6. o que você achou (R23) --------------------------------------
    let notas: Vec<(String, i32, Option<String>)> = sqlx::query_as(
        "SELECT w.title, a.nota, w.artwork->>'poster'
         FROM avaliacao a JOIN work w ON w.id = a.work_id
         WHERE a.user_id = $1
         ORDER BY a.nota DESC, a.atualizado_em DESC
         LIMIT $2",
    )
    .bind(user.id)
    .bind(NO_BLOCO as i64)
    .fetch_all(&state.pool)
    .await?;

    if !notas.is_empty() {
        let media: f32 =
            notas.iter().map(|(_, n, _)| *n as f32).sum::<f32>() / notas.len() as f32;
        blocos.push(Bloco {
            chave: "notas",
            titulo: "O que você achou",
            frase: format!("Você avaliou {}, e sua média é {media:.1}.", obras(notas.len())),
            detalhe: notas
                .into_iter()
                .map(|(titulo, nota, poster)| Item {
                    rotulo: titulo,
                    nota: Some("★".repeat(nota as usize)),
                    imagem: poster,
                })
                .collect(),
        });
    } else {
        calados += 1;
    }

    let desde: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT min(created_at) FROM play_event WHERE user_id = $1")
            .bind(user.id)
            .fetch_one(&state.pool)
            .await
            .unwrap_or(None);

    Ok(Json(Retrospectiva { blocos, calados, desde }))
}

fn obras(n: usize) -> String {
    if n == 1 { "1 obra".into() } else { format!("{n} obras") }
}

fn caixas(n: i64) -> String {
    if n == 1 { "1 caixa".into() } else { format!("{n} caixas") }
}

/// A hora de pico, quando há histograma. `None` quando tudo é zero — que é
/// diferente de "meia-noite", e é a diferença que impede a tela de afirmar que
/// você assiste de madrugada quando ela não sabe de nada.
fn pico_do_dia(histograma: &[f32]) -> Option<usize> {
    let (hora, valor) = histograma
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))?;
    (*valor > 0.0).then_some(hora)
}

/// A hora vira faixa do dia: "às 23h" é preciso e não é verdade — ninguém
/// assiste sempre na mesma hora. O pico diz o **período**.
fn faixa_do_dia(hora: usize) -> &'static str {
    match hora {
        5..=11 => "de manhã",
        12..=17 => "à tarde",
        18..=22 => "à noite",
        _ => "de madrugada",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A defesa central da R24 (§40): **bloco sem material não existe.**
    ///
    /// Sem isto, a retrospectiva deste acervo anunciaria "0 empréstimos · 0
    /// notas" — que é a tela "confiante sobre o que não sabia" da R15 (§26),
    /// de novo, e desta vez na tela mais pessoal do produto.
    #[test]
    fn histograma_zerado_nao_vira_madrugada() {
        assert_eq!(pico_do_dia(&[0.0; 24]), None);
        assert_eq!(pico_do_dia(&[]), None);

        // Com material, o pico é o pico — e a hora 0 é legítima quando é ela.
        let mut h = [0.0f32; 24];
        h[0] = 0.9;
        assert_eq!(pico_do_dia(&h), Some(0));
        h[20] = 1.0;
        assert_eq!(pico_do_dia(&h), Some(20));
    }

    /// A hora vira **período**, não relógio. "Às 23h" seria preciso e falso:
    /// ninguém assiste sempre na mesma hora, e o histograma tem um pico, não
    /// um compromisso.
    #[test]
    fn a_hora_vira_periodo_do_dia() {
        assert_eq!(faixa_do_dia(7), "de manhã");
        assert_eq!(faixa_do_dia(14), "à tarde");
        assert_eq!(faixa_do_dia(21), "à noite");
        assert_eq!(faixa_do_dia(2), "de madrugada");
        assert_eq!(faixa_do_dia(23), "de madrugada");
    }

    /// Singular e plural, porque "1 obras" numa tela que fala sobre você é o
    /// tipo de descuido que faz a frase inteira parecer gerada.
    #[test]
    fn a_frase_concorda_em_numero() {
        assert_eq!(obras(1), "1 obra");
        assert_eq!(obras(18), "18 obras");
        assert_eq!(caixas(1), "1 caixa");
        assert_eq!(caixas(3), "3 caixas");
    }
}
