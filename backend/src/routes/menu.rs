//! R21 — o menu de DVD.
//!
//! ## O que a medição decidiu, antes de qualquer tela
//!
//! O `IDEIAS.md` §3 mandava medir a cobertura de capítulos antes de desenhar.
//! Medido nos 548 filmes identificados deste acervo:
//!
//! | | |
//! |---|---|
//! | com capítulos | **74 — 13,5%** |
//! | com **nomes** de capítulo úteis | **9 — 1,6%** |
//! | com folha de sprites (§8d), o "plano B" previsto | **0 — as 725 folhas são de episódio e YouTube** |
//!
//! Duas conclusões, e as duas mudam o desenho:
//!
//! **1. Um menu de capítulos feito de nomes está morto.** Ele funcionaria em
//! nove filmes. Os "títulos" dos outros são vazios, `Chapter 01`, ou — pior e
//! mais comum — o próprio timecode repetido no campo de nome.
//!
//! **2. O plano B não existia.** O §3 assumia a folha de sprites como saída, e
//! ela cobre 725 arquivos — nenhum deles filme. Gerá-la custa **412 s por
//! filme**, porque varre o arquivo inteiro.
//!
//! Então a saída veio de outro fato já medido pelo projeto: **`-ss` antes do
//! `-i` é seek instantâneo** (§8g). Extrair um quadro no minuto 30 custa
//! **724 ms**; doze quadros custam ~6 s. Setecentas vezes mais barato que
//! varrer, e o suficiente pra uma grade de cenas.
//!
//! Daí o desenho: **a grade de cenas é o principal, e o capítulo é uma âncora
//! melhor quando existe.** Isso não é degradação — é o que "scene selection"
//! sempre foi num DVD: uma grade de miniaturas com timecode.
//!
//! ## O que este módulo não faz
//!
//! Não gera nada que ninguém pediu. A grade de cenas só é extraída quando
//! alguém **entra** na tela de cenas — que num DVD também era um item de menu,
//! e também demorava um instante pra carregar. O menu principal abre na hora.

use std::path::PathBuf;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Quantas cenas a grade mostra.
///
/// Doze é a grade 4×3 que todo menu de DVD usava, e é o que cabe numa tela sem
/// virar contato de fotógrafo. Também é o teto do custo: doze extrações de
/// ~724 ms, pagas uma vez por filme e guardadas em disco.
const CENAS: usize = 12;

/// Largura da miniatura de cena. 320px cobre a célula da grade em tela cheia
/// sem que a folha inteira passe de ~40 KB.
const LARGURA_CENA: u32 = 320;

/// Um capítulo, como o container o declara.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capitulo {
    pub inicio: f64,
    pub fim: f64,
    /// O nome, **quando ele é um nome**. Em 1,6% dos filmes deste acervo ele é;
    /// nos outros é vazio, `Chapter 01`, ou o próprio timecode — e nesses casos
    /// vale `None`, porque exibir "00:12:46" como nome de capítulo é mentir com
    /// cara de metadado (§18). A tela mostra o timecode de propósito, e aí ele
    /// é o timecode.
    pub titulo: Option<String>,
}

/// Uma cena da grade.
#[derive(Debug, Clone, Serialize)]
pub struct Cena {
    pub segundos: f64,
    /// Caminho relativo servido por `/artwork` — o mesmo lugar dos pôsteres,
    /// porque é o mesmo tipo de coisa: imagem derivada, cacheada em disco,
    /// descartável.
    pub imagem: String,
    /// De onde a cena saiu: `capitulo` quando o container disse onde ela
    /// começa, `regular` quando foi o relógio que dividiu. A tela não usa isto
    /// pra mudar o desenho — usa pra dizer a verdade na legenda.
    pub origem: &'static str,
}

#[derive(Debug, Serialize)]
pub struct MenuDoDisco {
    pub work_id: Uuid,
    pub media_file_id: Uuid,
    pub titulo: String,
    pub ano: Option<i32>,
    /// A cor da obra, do M1. É ela que tinge o menu — cada disco com a sua.
    pub cor: Option<String>,
    pub backdrop: Option<String>,
    pub duracao: Option<f64>,
    /// Onde você parou. `Some(_)` acende o "continuar"; `None` some com ele,
    /// em vez de deixar um item morto no menu (§24).
    pub posicao: Option<f64>,
    pub terminado: bool,
    pub capitulos: Vec<Capitulo>,
    /// Os idiomas de legenda **distintos** que o disco carrega.
    ///
    /// `subtitle_langs` é uma faixa por linha, e *Independence Day* traz 28
    /// delas — `por, por, eng, spa, spa, fre, fre…`. Um menu que lista o mesmo
    /// idioma cinco vezes está mostrando faixas, não idiomas, e a pergunta que
    /// alguém faz na frente de um menu é *"tem português?"*.
    ///
    /// **E o menu não escolhe legenda.** Ele diz o que o disco tem; escolher
    /// continua no player, onde já funciona desde o §18. Duplicar o seletor
    /// aqui seria dois lugares pra manter em sincronia por uma escolha que já
    /// tem dono.
    pub legendas: Vec<String>,
    /// O offset de onde a cena de fundo começa.
    ///
    /// **Sorteado a cada abertura** (R31). Era um quinto do filme, fixo, e a
    /// anotação original pede *"uma cena aleatória do filme rodando de fundo"* —
    /// com offset fixo, abrir o mesmo disco dez vezes dá o mesmo plano dez
    /// vezes, e um menu que nunca muda é um pôster com botões.
    ///
    /// O sorteio é no **miolo**: entre 15% e 75% da duração. Fora disso o menu
    /// mostraria o logo do estúdio (que não é o filme) ou o terceiro ato (que
    /// entrega o final de graça). A janela é a mesma decisão de antes, agora
    /// com largura em vez de um ponto.
    pub cena_de_fundo: f64,
    /// O **clima** do disco: o índice da estante que reivindicaria este filme
    /// na locadora, e o nome dela.
    ///
    /// Era um gênero cru, escolhido por um `SELECT … LIMIT 1` **sem ordenação**
    /// — o Postgres devolvia qualquer uma das até seis etiquetas do filme, e o
    /// sintetizador reduzia isso a três variantes com dois regex sobrepostos. O
    /// resultado é o defeito relatado: *a música é igual em todos os filmes*.
    ///
    /// A locadora já tinha resolvido exatamente este problema. `ESTANTES` é uma
    /// lista **ordenada** de reivindicação, com os gêneros distintivos primeiro,
    /// e é por isso que *Alien* vai pra ficção científica em vez de drama. Usar
    /// a mesma ordem aqui custa nada e rende coerência: o filme que mora na
    /// estante de terror abre um menu de terror.
    pub clima: i32,
    pub clima_nome: String,
}

/// Tudo que o menu precisa, numa requisição.
///
/// Uma e não quatro: o menu abre com o disco na mão e não pode ficar montando
/// a si mesmo em etapas. É o mesmo movimento do guia (§30) e das estantes
/// (§36).
pub async fn menu(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<MenuDoDisco>> {
    // R26: o menu e as cenas são quadros do filme. Um convidado que não pegou
    // a caixa não recebe doze miniaturas dela — seria o acervo vazando em
    // resolução baixa.
    if !crate::auth::acesso::pode_assistir_obra(&state.pool, &user, work_id).await {
        return Err(crate::auth::acesso::negado());
    }

    #[derive(sqlx::FromRow)]
    struct Linha {
        media_file_id: Uuid,
        path: String,
        titulo: String,
        ano: Option<i32>,
        cor: Option<String>,
        backdrop: Option<String>,
        duracao: Option<f64>,
        posicao: Option<f64>,
        terminado: Option<bool>,
        legendas: Vec<String>,
        chapters: Option<Value>,
        clima: Option<i32>,
    }

    let linha: Option<Linha> = sqlx::query_as(
        "SELECT m.id AS media_file_id, m.path,
                w.title AS titulo, w.year AS ano, w.dominant_color AS cor,
                w.artwork->>'backdrop' AS backdrop,
                COALESCE(m.duration_seconds, w.runtime_seconds::float8) AS duracao,
                ps.position_seconds AS posicao,
                ps.finished AS terminado,
                m.subtitle_langs AS legendas,
                m.chapters,
                -- **A mesma reivindicação da locadora**, e não uma etiqueta
                -- qualquer: a primeira estante (na ordem de `ESTANTES`) que
                -- reclama este filme. `min(idx)` é literalmente o `min(e.idx)`
                -- do `atribuicao` do §36, lido para uma obra só.
                (SELECT min(e.idx) FROM work_tag wt
                   JOIN tag t ON t.id = wt.tag_id AND t.namespace = 'genre'
                   JOIN LATERAL (SELECT * FROM unnest($3::int[], $4::text[]) AS x(idx, genero)) e
                     ON t.value = e.genero
                  WHERE wt.work_id = w.id) AS clima
         FROM work w
         JOIN LATERAL (
             SELECT m.* FROM media_file m
             WHERE m.work_id = w.id AND m.status = 'probed'
             ORDER BY m.size_bytes DESC LIMIT 1
         ) m ON true
         LEFT JOIN playback_state ps ON ps.work_id = w.id AND ps.user_id = $2
         WHERE w.id = $1",
    )
    .bind(work_id)
    .bind(user.id)
    .bind(indices_das_estantes())
    .bind(generos_das_estantes())
    .fetch_optional(&state.pool)
    .await?;

    let linha = linha.ok_or(AppError::NotFound)?;

    // NULL é "nunca perguntei"; `[]` é "perguntei e não tem". Sem a distinção,
    // os 474 filmes sem capítulo seriam reprobados a cada abertura.
    let capitulos = match linha.chapters {
        Some(v) => serde_json::from_value(v).unwrap_or_default(),
        None => {
            let lidos = ler_capitulos(&linha.path).await;
            let _ = sqlx::query("UPDATE media_file SET chapters = $2 WHERE id = $1")
                .bind(linha.media_file_id)
                .bind(serde_json::to_value(&lidos).unwrap_or_else(|_| serde_json::json!([])))
                .execute(&state.pool)
                .await;
            lidos
        }
    };

    let duracao = linha.duracao;
    // Sem gênero nenhum, cai na última estante — a mesma que a locadora usa
    // como sumidouro. Um filme sem etiqueta tem que abrir **algum** menu, e o
    // drama é o clima mais neutro da lista.
    let clima = linha
        .clima
        .unwrap_or(crate::routes::locadora::ESTANTES.len() as i32 - 1);

    Ok(Json(MenuDoDisco {
        work_id,
        media_file_id: linha.media_file_id,
        titulo: linha.titulo,
        ano: linha.ano,
        cor: linha.cor,
        backdrop: linha.backdrop,
        duracao,
        // Posição zero não é "continuar de onde parou" — é o começo, e o menu
        // já tem um item pra isso.
        posicao: linha.posicao.filter(|p| *p > 30.0),
        terminado: linha.terminado.unwrap_or(false),
        capitulos,
        legendas: distintos(linha.legendas),
        cena_de_fundo: cena_sorteada(duracao),
        clima,
        clima_nome: crate::routes::locadora::ESTANTES
            .get(clima as usize)
            .map_or_else(|| "Drama".to_string(), |(nome, _)| nome.to_string()),
    }))
}

/// Onde a cena de fundo começa, sorteada no miolo do filme.
///
/// **Entre 15% e 75%.** Antes dos 15% ainda há logo de estúdio e créditos de
/// abertura, e um menu que mostra o logo atrás não está mostrando o filme;
/// depois dos 75% começa o desfecho, e um menu não deve entregar o final de
/// graça. A janela é a mesma decisão do offset fixo — agora com largura.
///
/// Sem duração conhecida, zero: sortear às cegas poria o menu no logo do
/// estúdio de qualquer jeito, e o começo pelo menos não finge ser escolha.
fn cena_sorteada(duracao: Option<f64>) -> f64 {
    use rand::Rng;
    match duracao {
        Some(d) if d > 60.0 => rand::rngs::OsRng.gen_range(0.15..0.75) * d,
        _ => 0.0,
    }
}

/// Os índices e os gêneros de `ESTANTES`, achatados pra virar uma tabela de
/// `unnest` na consulta.
///
/// Uma estante reivindica vários rótulos crus (o acervo tem dois vocabulários,
/// §36), então cada par (índice, gênero) vira uma linha. Achatar aqui e mandar
/// dois arrays por bind evita construir SQL com o conteúdo da constante.
fn indices_das_estantes() -> Vec<i32> {
    crate::routes::locadora::ESTANTES
        .iter()
        .enumerate()
        .flat_map(|(i, (_, generos))| generos.iter().map(move |_| i as i32))
        .collect()
}

fn generos_das_estantes() -> Vec<String> {
    crate::routes::locadora::ESTANTES
        .iter()
        .flat_map(|(_, generos)| generos.iter().map(|g| g.to_string()))
        .collect()
}

/// Idiomas distintos, **na ordem em que aparecem**.
///
/// Ordenar alfabeticamente jogaria português pro meio da lista num acervo em
/// que ele é quase sempre a primeira faixa — e a ordem das faixas é uma
/// informação do disco, não ruído.
fn distintos(langs: Vec<String>) -> Vec<String> {
    let mut vistos = std::collections::HashSet::new();
    langs
        .into_iter()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| vistos.insert(l.to_lowercase()))
        .collect()
}

/// Lê os capítulos do container.
///
/// `ffprobe -show_chapters` custa 242 ms, e é o mesmo binário que o scanner já
/// usa. Não entra no `probe` do scanner de propósito: passar 17.498 arquivos
/// pra preencher uma coluna que só interessa a 548 filmes seria pagar caro no
/// lugar errado.
async fn ler_capitulos(path: &str) -> Vec<Capitulo> {
    let saida = tokio::process::Command::new("ffprobe")
        .args([
            "-v", "quiet", "-print_format", "json", "-show_chapters", path,
        ])
        .output()
        .await;

    let Ok(saida) = saida else { return Vec::new() };
    let Ok(json) = serde_json::from_slice::<Value>(&saida.stdout) else {
        return Vec::new();
    };

    json.get("chapters")
        .and_then(|c| c.as_array())
        .map(|cs| {
            cs.iter()
                .filter_map(|c| {
                    let inicio = c.get("start_time")?.as_str()?.parse().ok()?;
                    let fim = c
                        .get("end_time")
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(inicio);
                    Some(Capitulo {
                        inicio,
                        fim,
                        titulo: c
                            .get("tags")
                            .and_then(|t| t.get("title"))
                            .and_then(|t| t.as_str())
                            .map(str::trim)
                            .filter(|t| e_nome_de_verdade(t))
                            .map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// O título do capítulo é um nome, ou é preenchimento?
///
/// Medido: dos 74 filmes com capítulo, **9** têm nome de verdade. O resto traz
/// vazio, `Chapter 01`, `Capítulo 03` — ou, o caso mais traiçoeiro, **o próprio
/// timecode** no campo de título. Exibir `00:12:46` como se fosse o nome do
/// capítulo é mentir com cara de metadado (§18): parece informação e é o
/// mesmo número que já está do lado.
fn e_nome_de_verdade(t: &str) -> bool {
    if t.is_empty() {
        return false;
    }
    let baixo = t.to_lowercase();
    // `Chapter 7`, `Capítulo 03`, `Part 2`, `Scene 01`, `Título 4`.
    let generico = ["chapter", "capítulo", "capitulo", "part", "scene", "cena", "título", "titulo"]
        .iter()
        .any(|p| {
            baixo
                .strip_prefix(p)
                .map(|r| r.trim().chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(false)
        });
    if generico {
        return false;
    }
    // Só dígitos, ou um timecode `00:12:46.474`.
    if baixo.chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    let so_tempo = baixo
        .chars()
        .all(|c| c.is_ascii_digit() || c == ':' || c == '.' || c == ',');
    !(so_tempo && baixo.contains(':'))
}

/// Onde as cenas de um arquivo ficam guardadas, dentro de `artwork_dir`.
fn pasta_de_cenas(state: &AppState, media_file_id: Uuid) -> PathBuf {
    state.config.artwork_dir.join("cenas").join(media_file_id.to_string())
}

/// A grade de cenas.
///
/// **Gerada sob demanda e guardada em disco.** Custa ~6 s na primeira vez e
/// nada nas seguintes — e é cobrada só de quem abriu a tela de cenas, que num
/// DVD também era um item de menu e também demorava um instante.
///
/// Os pontos saem dos capítulos quando eles existem (13,5% dos filmes) e do
/// relógio quando não. A tela é a mesma nos dois casos, e isso é correto: um
/// menu de DVD mostrava a mesma grade de miniaturas tivesse o disco capítulos
/// nomeados ou não.
pub async fn cenas(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(work_id): Path<Uuid>,
) -> AppResult<Json<Vec<Cena>>> {
    // R26: o menu e as cenas são quadros do filme. Um convidado que não pegou
    // a caixa não recebe doze miniaturas dela — seria o acervo vazando em
    // resolução baixa.
    if !crate::auth::acesso::pode_assistir_obra(&state.pool, &user, work_id).await {
        return Err(crate::auth::acesso::negado());
    }

    #[derive(sqlx::FromRow)]
    struct Fonte {
        id: Uuid,
        path: String,
        duracao: Option<f64>,
        chapters: Option<Value>,
    }

    let fonte: Fonte = sqlx::query_as(
        "SELECT m.id, m.path,
                COALESCE(m.duration_seconds, w.runtime_seconds::float8) AS duracao,
                m.chapters
         FROM work w
         JOIN LATERAL (
             SELECT m.* FROM media_file m
             WHERE m.work_id = w.id AND m.status = 'probed'
             ORDER BY m.size_bytes DESC LIMIT 1
         ) m ON true
         WHERE w.id = $1",
    )
    .bind(work_id)
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let duracao = fonte
        .duracao
        .filter(|d| *d > 60.0)
        .ok_or_else(|| AppError::BadRequest("arquivo sem duração conhecida".into()))?;

    let capitulos: Vec<Capitulo> = fonte
        .chapters
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();

    let (pontos, origem) = pontos_das_cenas(&capitulos, duracao);

    let dir = pasta_de_cenas(&state, fonte.id);
    tokio::fs::create_dir_all(&dir).await.ok();

    // As doze extrações vão juntas. São processos independentes lendo o mesmo
    // arquivo — e o gargalo é I/O, então o ganho é modesto (6 s → 4 s), mas é
    // de graça.
    let mut tarefas = Vec::new();
    for (i, t) in pontos.iter().enumerate() {
        let destino = dir.join(format!("{i:02}.jpg"));
        let path = fonte.path.clone();
        let t = *t;
        tarefas.push(tokio::spawn(async move {
            if tokio::fs::metadata(&destino).await.is_ok() {
                return true; // já extraída numa visita anterior
            }
            extrair_quadro(&path, t, &destino).await
        }));
    }

    let mut cenas = Vec::new();
    for (i, (t, tarefa)) in pontos.iter().zip(tarefas).enumerate() {
        // Quadro que não saiu simplesmente não vira célula. Uma célula preta
        // com "indisponível" seria pior que uma grade de onze (§24).
        if tarefa.await.unwrap_or(false) {
            cenas.push(Cena {
                segundos: *t,
                imagem: format!("cenas/{}/{i:02}.jpg", fonte.id),
                origem,
            });
        }
    }

    Ok(Json(cenas))
}

/// Onde cortar as cenas, e por quê.
///
/// Com capítulos: os começos deles, amostrados em passo regular quando são
/// mais que doze — 94 capítulos (o recorde deste acervo) não cabem numa grade,
/// e mostrar só os doze primeiros daria doze cenas do primeiro ato.
///
/// Sem capítulos: passo regular, **começando depois da abertura e parando
/// antes do fim**. Os 4% iniciais são logo de estúdio e os 4% finais são
/// créditos — nenhum dos dois é uma cena, e o segundo estraga o filme.
fn pontos_das_cenas(capitulos: &[Capitulo], duracao: f64) -> (Vec<f64>, &'static str) {
    if capitulos.len() >= 2 {
        let uteis: Vec<f64> = capitulos
            .iter()
            .map(|c| c.inicio)
            // Capítulo que começa em zero é a abertura em todo disco.
            .filter(|t| *t > 1.0 && *t < duracao)
            .collect();
        if !uteis.is_empty() {
            let passo = (uteis.len() as f64 / CENAS as f64).max(1.0);
            let escolhidos: Vec<f64> = (0..CENAS.min(uteis.len()))
                .map(|i| uteis[((i as f64) * passo) as usize])
                .collect();
            return (escolhidos, "capitulo");
        }
    }

    let inicio = duracao * 0.04;
    let fim = duracao * 0.96;
    let passo = (fim - inicio) / CENAS as f64;
    (
        (0..CENAS).map(|i| inicio + passo * i as f64).collect(),
        "regular",
    )
}

/// Um quadro, no ponto pedido.
///
/// `-ss` **antes** do `-i`: é o seek instantâneo que o §8g mediu, e é a
/// diferença entre 724 ms e varrer o arquivo. Com `-ss` depois do `-i` o
/// ffmpeg decodifica tudo até chegar lá, que foi como a folha de sprites (§8d)
/// acabou custando 412 s por filme.
async fn extrair_quadro(path: &str, segundos: f64, destino: &std::path::Path) -> bool {
    let saida = tokio::process::Command::new("ffmpeg")
        .args([
            "-v", "quiet",
            "-ss", &format!("{segundos:.3}"),
            "-i", path,
            "-frames:v", "1",
            "-vf", &format!("scale={LARGURA_CENA}:-2"),
            "-q:v", "6",
            "-y",
        ])
        .arg(destino)
        .output()
        .await;

    matches!(saida, Ok(s) if s.status.success())
        && tokio::fs::metadata(destino).await.map(|m| m.len() > 0).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    /// **Os dois arrays do clima andam juntos.** Eles viram uma tabela de
    /// `unnest($3, $4)` na consulta, e `unnest` de dois arrays de tamanhos
    /// diferentes preenche o menor com NULL em silêncio — o resultado seria uma
    /// estante casando com o gênero errado, e ninguém notaria: o menu abriria,
    /// só com a música de outro filme.
    #[test]
    fn os_dois_arrays_do_clima_tem_o_mesmo_tamanho() {
        let idx = super::indices_das_estantes();
        let gen = super::generos_das_estantes();
        assert_eq!(idx.len(), gen.len());
        assert!(!idx.is_empty());
        // E o achatamento preserva a ordem: o primeiro par é a primeira
        // estante com o primeiro rótulo dela.
        assert_eq!(idx[0], 0);
        assert_eq!(gen[0], crate::routes::locadora::ESTANTES[0].1[0]);
        // O último índice é o da última estante — se isto cair, algum `flat_map`
        // perdeu uma estante inteira e os filmes dela viram drama.
        assert_eq!(
            *idx.last().unwrap(),
            crate::routes::locadora::ESTANTES.len() as i32 - 1
        );
    }

    /// A cena de fundo é **sorteada no miolo**, e é uma janela, não um ponto.
    ///
    /// Antes dos 15% ainda há logo de estúdio; depois dos 75% começa o
    /// desfecho. Se este teste cair pra fora, o menu passa a mostrar o final do
    /// filme atrás dos botões.
    #[test]
    fn a_cena_de_fundo_cai_no_miolo() {
        let d = 6000.0;
        for _ in 0..50 {
            let c = super::cena_sorteada(Some(d));
            assert!(c >= 0.15 * d, "cena cedo demais: {c}");
            assert!(c < 0.75 * d, "cena tarde demais: {c}");
        }
        // Duas aberturas seguidas não dão o mesmo plano — é o defeito que a
        // R31 conserta, e sem esta linha ele volta sem quebrar nada.
        let a = super::cena_sorteada(Some(d));
        let b = super::cena_sorteada(Some(d));
        assert_ne!(a, b, "a cena voltou a ser determinística");
        // Sem duração conhecida não há sorteio: zero é honesto, chute não.
        assert_eq!(super::cena_sorteada(None), 0.0);
        assert_eq!(super::cena_sorteada(Some(10.0)), 0.0);
    }

    use super::*;

    fn cap(inicio: f64, titulo: Option<&str>) -> Capitulo {
        Capitulo { inicio, fim: inicio + 60.0, titulo: titulo.map(str::to_string) }
    }

    /// Medido: dos 74 filmes com capítulo, 9 têm nome de verdade. O caso mais
    /// traiçoeiro é o **timecode no campo de título** — ele parece informação e
    /// é o mesmo número que já está do lado. Exibi-lo seria o "inglês" chutado
    /// que o §18 recusa.
    #[test]
    fn timecode_nao_e_nome_de_capitulo() {
        assert!(!e_nome_de_verdade("00:12:46.474"));
        assert!(!e_nome_de_verdade("0:00:00"));
        assert!(!e_nome_de_verdade(""));
        assert!(!e_nome_de_verdade("Chapter 01"));
        assert!(!e_nome_de_verdade("Capítulo 3"));
        assert!(!e_nome_de_verdade("Scene 01"));
        assert!(!e_nome_de_verdade("7"));

        assert!(e_nome_de_verdade("Main Titles / Death's Design"));
        assert!(e_nome_de_verdade("Deadly Commute"));
        // Um nome que CONTÉM número continua sendo nome.
        assert!(e_nome_de_verdade("Apartamento 4B"));
    }

    /// Sem capítulos — 86,5% do acervo — o passo é regular e **não pega os
    /// extremos**: os primeiros 4% são logo de estúdio e os últimos 4% são
    /// créditos. Um deles não é cena; o outro entrega o final.
    #[test]
    fn sem_capitulos_o_passo_e_regular_e_evita_os_extremos() {
        let (pontos, origem) = pontos_das_cenas(&[], 6000.0);
        assert_eq!(origem, "regular");
        assert_eq!(pontos.len(), CENAS);
        assert!(pontos[0] >= 6000.0 * 0.04, "começou na abertura: {}", pontos[0]);
        assert!(
            *pontos.last().unwrap() <= 6000.0 * 0.96,
            "chegou nos créditos: {}",
            pontos.last().unwrap()
        );
        // Estritamente crescente: duas cenas no mesmo ponto seriam a mesma
        // miniatura duas vezes.
        assert!(pontos.windows(2).all(|w| w[1] > w[0]));
    }

    /// Com capítulos, eles mandam — mas **amostrados**. O recorde deste acervo
    /// é 94 capítulos: pegar os doze primeiros daria doze cenas do primeiro ato.
    #[test]
    fn muitos_capitulos_sao_amostrados_ao_longo_do_filme() {
        let muitos: Vec<Capitulo> = (1..=94).map(|i| cap(i as f64 * 60.0, None)).collect();
        let (pontos, origem) = pontos_das_cenas(&muitos, 5700.0);
        assert_eq!(origem, "capitulo");
        assert_eq!(pontos.len(), CENAS);
        assert!(
            *pontos.last().unwrap() > 4000.0,
            "as 12 cenas ficaram todas no começo: {:?}",
            pontos
        );
        assert!(pontos.windows(2).all(|w| w[1] > w[0]));
    }

    /// O capítulo que começa em zero é a abertura, em todo disco — ele não
    /// rende cena, rende a tela preta antes do logo.
    #[test]
    fn o_capitulo_zero_nao_vira_cena() {
        let cs = vec![cap(0.0, None), cap(600.0, None), cap(1200.0, None)];
        let (pontos, _) = pontos_das_cenas(&cs, 3600.0);
        assert!(!pontos.contains(&0.0), "a abertura virou cena: {pontos:?}");
    }

    /// Um capítulo só não é uma grade — cai no passo regular, que dá doze.
    #[test]
    fn um_capitulo_so_cai_no_regular() {
        let (_, origem) = pontos_das_cenas(&[cap(0.0, None)], 3600.0);
        assert_eq!(origem, "regular");
    }

    /// *Independence Day* traz 28 faixas de legenda e **8 idiomas**. Listar as
    /// 28 mostraria faixas; a pergunta na frente de um menu é "tem português?".
    #[test]
    fn legendas_repetidas_viram_idiomas_distintos() {
        let cru = ["por", "por", "eng", "spa", "spa", "fre", "fre", "spa", "dan", "", "ENG"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(distintos(cru), vec!["por", "eng", "spa", "fre", "dan"]);
    }

    /// A ordem é a do disco, não a do dicionário: neste acervo o português é
    /// quase sempre a primeira faixa, e ordenar o jogaria pro meio.
    #[test]
    fn a_ordem_das_faixas_e_preservada() {
        let cru = ["swe", "por", "eng"].iter().map(|s| s.to_string()).collect();
        assert_eq!(distintos(cru), vec!["swe", "por", "eng"]);
    }
}
