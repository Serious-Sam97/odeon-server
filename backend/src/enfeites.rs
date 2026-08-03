//! R43 — os enfeites do perfil: rosto, capa e moldura.
//!
//! ## A decisão que mudou o desenho
//!
//! O `IDEIAS-2.md` §4.1 propôs avatares **desenhados em SVG**, pela régua de
//! zero bytes do §12. Quem decide vetou e pediu outra coisa:
//!
//! > *"o rosto de alguns atores, diretores, pessoal de música… e algumas capas
//! > que são capas usadas em próprios filmes"*
//!
//! É melhor, e por um motivo que a proposta original não tinha visto: **a arte
//! já está no disco**. Medido: **5.606 atores, 381 diretores e 249
//! compositores** deste acervo têm rosto no cache local (`person.image_path`,
//! baixado pelo pipeline do M2), e todo filme identificado tem backdrop.
//!
//! Ou seja, o rosto não custa um byte novo a servir — é o mesmo `/artwork/…`
//! que a ficha já usa. Fica zero bytes **e** fica do acervo de quem olha.
//!
//! ## O catálogo é código, e o vínculo é temático
//!
//! Mesma decisão da lista de conquistas (§48): *"quem escreve a lista é quem
//! programa"*. Uma tabela daria uma tela de administração que ninguém pediu, e
//! transformaria em dado uma regra que é do programa.
//!
//! O que se guarda no banco é a **escolha** — uma chave de texto por pessoa.
//!
//! ## Quem não está no acervo não aparece
//!
//! Cada entrada aponta pra uma pessoa **pelo nome** e pra um filme **pelo
//! título**. Se a pessoa não existe neste servidor, a opção some da lista em
//! vez de virar uma moldura vazia — é o §18: a tela omite em vez de chutar.
//!
//! Isso torna o catálogo portátil de graça: o mesmo código num acervo com
//! outros filmes oferece outros rostos, sem migração e sem erro.

use serde::Serialize;
use sqlx::PgPool;
use std::collections::HashMap;

/// Um enfeite do catálogo, antes de ser resolvido contra o acervo.
pub struct Enfeite {
    /// A chave guardada no banco.
    pub chave: &'static str,
    /// O nome da pessoa (rosto) ou o título do filme (capa), como o TMDB os
    /// grava em pt-BR — é por ele que a busca acha a linha.
    pub alvo: &'static str,
    /// A conquista que abre. `None` nasce aberto.
    ///
    /// **Metade aberta**, como o §4.1 pediu: um perfil que começa sem nenhuma
    /// opção é uma tela de escolher que não deixa escolher nada.
    pub exige: Option<&'static str>,
    /// Só a moldura tem: o hex que ela pinta.
    ///
    /// Separado do `alvo` porque **a tela mostra o nome e pinta a cor**, e as
    /// duas coisas são diferentes: um seletor que diz "#4ea36b — abre com
    /// Sócio" está mostrando endereço de memória pra quem escolhe cor.
    pub cor: Option<&'static str>,
}

const fn e(chave: &'static str, alvo: &'static str, exige: Option<&'static str>) -> Enfeite {
    Enfeite { chave, alvo, exige, cor: None }
}

const fn m(
    chave: &'static str,
    nome: &'static str,
    cor: &'static str,
    exige: Option<&'static str>,
) -> Enfeite {
    Enfeite { chave, alvo: nome, exige, cor: Some(cor) }
}

/// Doze rostos. Seis abertos, seis atrás de conquista.
///
/// **O vínculo é temático, e é o que faz ele valer alguma coisa**: o rosto da
/// Sigourney Weaver abre com dez de ficção científica, o do Tarantino com dez
/// de crime. Uma conquista sorteada daria a mesma medalha sem dizer nada sobre
/// quem a tem.
pub const ROSTOS: &[Enfeite] = &[
    // Abertos — os seis com mais presença neste acervo, pra a tela de escolher
    // nunca abrir vazia.
    e("robin_williams", "Robin Williams", None),
    e("harrison_ford", "Harrison Ford", None),
    e("jim_carrey", "Jim Carrey", None),
    e("samuel_jackson", "Samuel L. Jackson", None),
    e("scarlett_johansson", "Scarlett Johansson", None),
    e("tom_hanks", "Tom Hanks", None),
    // Atrás de conquista.
    e("sigourney_weaver", "Sigourney Weaver", Some("scifi10")),
    e("quentin_tarantino", "Quentin Tarantino", Some("crime10")),
    e("david_fincher", "David Fincher", Some("madrugada")),
    e("uma_thurman", "Uma Thurman", Some("maratona6")),
    e("steven_spielberg", "Steven Spielberg", Some("sete_decadas")),
    e("hans_zimmer", "Hans Zimmer", Some("cem")),
];

/// Seis capas, todas de filme deste acervo. Três abertas, três atrás de
/// conquista — e a conquista é do gênero do filme.
pub const CAPAS: &[Enfeite] = &[
    e("volta_para_o_futuro", "De Volta para o Futuro", None),
    e("matrix", "Matrix", None),
    e("drive", "Drive", None),
    e("akira", "Akira", Some("anim10")),
    e("corra", "Corra!", Some("terror10")),
    e("duna", "Duna", Some("scifi50")),
];

/// A cor do perfil.
///
/// Aqui o alvo não é o acervo: é a própria cor, em hex. Quatro e não doze — a
/// moldura tinge o perfil inteiro, e doze variações do mesmo perfil não são
/// doze escolhas, são doze tons.
pub const MOLDURAS: &[Enfeite] = &[
    m("ambar", "Âmbar", "#e0b062", None),
    m("projetor", "Projetor", "#4ea36b", Some("vinte")),
    m("cortina", "Cortina", "#a8324a", Some("cem")),
    m("madrugada", "Madrugada", "#5b7f95", Some("madrugada")),
];

/// Um enfeite pronto pra tela: com a arte resolvida e o rótulo legível.
#[derive(Debug, Clone, Serialize)]
pub struct EnfeiteNaTela {
    pub chave: &'static str,
    /// O nome da pessoa, o título do filme, ou o hex da cor.
    pub rotulo: &'static str,
    /// O caminho servível em `/artwork/…`. `None` na moldura, que é só cor.
    pub arte: Option<String>,
    /// A conquista que abre, quando há uma.
    pub exige: Option<&'static str>,
    /// O nome legível dessa conquista — a tela não traduz chave (§48).
    pub exige_nome: Option<&'static str>,
    /// O hex, quando é moldura.
    pub cor: Option<&'static str>,
    /// Se quem está olhando já pode usar.
    pub aberto: bool,
}

/// Resolve os três catálogos contra o acervo e contra o que a pessoa
/// desbloqueou.
///
/// **Duas consultas para os três catálogos**, e não uma por entrada: são
/// dezoito linhas de catálogo, e dezoito idas ao banco por abertura de perfil
/// seria o mesmo erro que o §48 evitou nas conquistas.
pub async fn disponiveis(
    pool: &PgPool,
    ja: &HashMap<String, chrono::DateTime<chrono::Utc>>,
) -> (Vec<EnfeiteNaTela>, Vec<EnfeiteNaTela>, Vec<EnfeiteNaTela>) {
    let rostos = resolver_rostos(pool).await;
    let capas = resolver_capas(pool).await;

    let monta = |cat: &'static [Enfeite], arte: &HashMap<&str, String>, com_arte: bool| {
        cat.iter()
            .filter_map(|x| {
                let a = arte.get(x.alvo).cloned();
                // Sem arte no acervo, a opção não existe (§18).
                if com_arte && a.is_none() {
                    return None;
                }
                Some(EnfeiteNaTela {
                    chave: x.chave,
                    rotulo: x.alvo,
                    arte: a,
                    exige: x.exige,
                    exige_nome: x.exige.and_then(nome_da_conquista),
                    cor: x.cor,
                    aberto: x.exige.is_none_or(|c| ja.contains_key(c)),
                })
            })
            .collect::<Vec<_>>()
    };

    let vazio = HashMap::new();
    (
        monta(ROSTOS, &rostos, true),
        monta(CAPAS, &capas, true),
        monta(MOLDURAS, &vazio, false),
    )
}

/// O rosto de cada pessoa do catálogo que existe neste acervo.
async fn resolver_rostos(pool: &PgPool) -> HashMap<&'static str, String> {
    let nomes: Vec<&str> = ROSTOS.iter().map(|x| x.alvo).collect();
    let linhas: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (name) name, image_path
         FROM person
         WHERE name = ANY($1) AND image_path IS NOT NULL
         ORDER BY name, updated_at DESC",
    )
    .bind(&nomes)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    casar(ROSTOS, linhas)
}

/// A arte de cada filme do catálogo que existe neste acervo.
///
/// **Backdrop, e não pôster**: uma capa de perfil é larga, e um pôster esticado
/// numa faixa de 200px de altura vira uma tarja borrada. Filme sem backdrop cai
/// no pôster em vez de sumir — melhor a capa certa mal enquadrada do que a
/// opção some sem explicação.
async fn resolver_capas(pool: &PgPool) -> HashMap<&'static str, String> {
    let titulos: Vec<&str> = CAPAS.iter().map(|x| x.alvo).collect();
    let linhas: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (title) title,
                COALESCE(artwork->>'backdrop', artwork->>'poster') AS arte
         FROM work
         WHERE kind = 'movie' AND title = ANY($1)
           AND (artwork ? 'backdrop' OR artwork ? 'poster')
         ORDER BY title, year",
    )
    .bind(&titulos)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    casar(CAPAS, linhas)
}

/// Casa o que voltou do banco (por nome) com as chaves do catálogo.
fn casar(cat: &'static [Enfeite], linhas: Vec<(String, String)>) -> HashMap<&'static str, String> {
    let achados: HashMap<String, String> = linhas.into_iter().collect();
    cat.iter()
        .filter_map(|x| achados.get(x.alvo).map(|arte| (x.alvo, arte.clone())))
        .collect()
}

/// O nome legível de uma conquista, pra tela poder dizer o que falta fazer.
fn nome_da_conquista(chave: &str) -> Option<&'static str> {
    crate::conquistas::LISTA
        .iter()
        .find(|(q, _)| q.chave == chave)
        .map(|(q, _)| q.nome)
}

/// O enfeite escolhido por alguém, já resolvido — é o que o perfil de outra
/// pessoa mostra.
///
/// Devolve `None` quando a escolha não resolve mais: um rosto de pessoa que
/// saiu do acervo vira ausência, e a tela cai na marca derivada do nome (R42)
/// em vez de mostrar uma moldura quebrada.
pub fn escolhido(catalogo: &[EnfeiteNaTela], chave: Option<&str>) -> Option<EnfeiteNaTela> {
    let chave = chave?;
    catalogo.iter().find(|x| x.chave == chave).cloned()
}

/// Se uma chave pode ser usada por quem tem estes desbloqueios.
///
/// **Recusa por ausência de arte também**, e não só por conquista: escolher um
/// rosto que este acervo não tem gravaria uma chave que nenhuma tela consegue
/// desenhar.
pub fn pode_usar(catalogo: &[EnfeiteNaTela], chave: &str) -> bool {
    catalogo.iter().any(|x| x.chave == chave && x.aberto)
}

/// A cor de uma moldura, pela chave. A tela recebe a cor pronta — traduzir
/// chave pra hex no front seria a mesma lista escrita duas vezes.
pub fn cor_da_moldura(chave: Option<&str>) -> Option<&'static str> {
    let chave = chave?;
    MOLDURAS.iter().find(|x| x.chave == chave).and_then(|x| x.cor)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **Metade aberta**, como foi pedido. Um catálogo todo trancado abre a tela
    /// de escolher sem nada pra escolher; um todo aberto não é recompensa
    /// nenhuma.
    #[test]
    fn metade_nasce_aberta() {
        for cat in [ROSTOS, CAPAS] {
            let abertos = cat.iter().filter(|x| x.exige.is_none()).count();
            assert_eq!(abertos * 2, cat.len(), "o catálogo deixou de ser meio a meio");
        }
    }

    /// Toda conquista citada por um enfeite **existe na lista**.
    ///
    /// Uma chave errada aqui trancaria o enfeite pra sempre, em silêncio: a
    /// conquista nunca seria desbloqueada porque ela não existe, e ninguém
    /// descobriria — é o §8b vestido de cosmético.
    #[test]
    fn toda_exigencia_e_uma_conquista_de_verdade() {
        for cat in [ROSTOS, CAPAS, MOLDURAS] {
            for x in cat {
                if let Some(c) = x.exige {
                    assert!(
                        nome_da_conquista(c).is_some(),
                        "{} exige a conquista inexistente {c}",
                        x.chave
                    );
                }
            }
        }
    }

    /// Chave duplicada faria duas opções gravarem o mesmo valor, e a segunda
    /// nunca seria escolhível.
    #[test]
    fn as_chaves_nao_se_repetem() {
        let mut todas: Vec<&str> = [ROSTOS, CAPAS, MOLDURAS]
            .iter()
            .flat_map(|cat| cat.iter().map(|x| x.chave))
            .collect();
        let antes = todas.len();
        todas.sort_unstable();
        todas.dedup();
        assert_eq!(antes, todas.len(), "há chave de enfeite repetida");
    }

    /// A moldura guarda **nome e cor**, e a tela recebe os dois: o nome pra
    /// ler, o hex pra pintar. Ter só o hex fazia o seletor dizer "#4ea36b —
    /// abre com Sócio" pra quem está escolhendo uma cor.
    #[test]
    fn moldura_e_hex() {
        for m in MOLDURAS {
            let cor = m.cor.expect("moldura sem cor");
            assert!(cor.starts_with('#') && cor.len() == 7, "{}", m.chave);
            assert!(!m.alvo.starts_with('#'), "{} mostra hex como nome", m.chave);
        }
        assert_eq!(cor_da_moldura(Some("ambar")), Some("#e0b062"));
        assert_eq!(cor_da_moldura(Some("nao-existe")), None);
        assert_eq!(cor_da_moldura(None), None);
    }
}

/// Chave do rosto → arte, pra uma lista de gente resolver vários de uma vez.
///
/// **Uma consulta pra lista inteira**, e não uma por pessoa: a sala de gente
/// mostra todo mundo do servidor, e resolver rosto por linha seria uma ida ao
/// banco por linha pra desenhar um retrato de 34px.
pub async fn arte_por_chave(pool: &PgPool) -> HashMap<&'static str, String> {
    let por_nome = resolver_rostos(pool).await;
    ROSTOS
        .iter()
        .filter_map(|x| por_nome.get(x.alvo).map(|arte| (x.chave, arte.clone())))
        .collect()
}
