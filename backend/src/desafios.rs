//! R35 — os desafios.
//!
//! O último dos onze itens, e o único que nunca tinha sido construído.
//!
//! > *"Tarefas com prazo, que dão experiência. Mais simples que os temas do
//! > guia, e sorteadas para cada pessoa — não são iguais pra todos. A cadência é
//! > escolhida pela pessoa, entre algumas opções definidas."*
//!
//! ## O oposto do guia, de propósito
//!
//! O §2.4 do `IDEIAS.md` separa as duas coisas com uma tabela de duas colunas:
//!
//! | coletivo, igual pra todos | individual, por pessoa |
//! |---|---|
//! | guia da semana · eventos | **desafios** · XP · conquistas |
//!
//! O guia (§50) é derivado e não guarda nada, porque é o mesmo pra todo mundo e
//! recalculável. O desafio guarda, porque a janela de cada um começa num
//! instante diferente e porque *"cumpriu dentro do prazo"* deixa de ser
//! recuperável quando o prazo passa.
//!
//! ## Três por janela, e eles fazem trabalhos diferentes
//!
//! | fatia | o que faz |
//! |---|---|
//! | **fácil** | dopamina, e não é só assistir: avaliar, alugar e escrever entram aqui |
//! | **tema** | um gênero, uma década, uma fita — algo específico do acervo |
//! | **empurrão** | o que você **nunca** viu: um país, um diretor, um gênero inédito |
//!
//! O terceiro é o único que faz o desafio servir ao terceiro pilar (§1): sem
//! ele, um sistema de tarefas sorteadas do seu próprio gosto só reforça o gosto.
//!
//! ## Falhar não custa nada
//!
//! A janela fecha, o desafio some, outro é sorteado. Sem perda de XP, sem
//! sequência quebrada, sem aviso.
//!
//! **Este projeto tem uma punição só, e ela é social** — a fita mal devolvida
//! (§46). Ela funciona porque é entre pessoas e porque o atrito é a graça. Punir
//! alguém por não ter assistido um filme é o placar do §40 mandando de novo, com
//! outra roupa.
//!
//! ## Uma propriedade que vale conhecer
//!
//! A geração é idempotente **enquanto o sorteio não muda**. Mexer nas listas
//! deste arquivo ou na semente faz a janela em curso ganhar desafios a mais —
//! os antigos continuam válidos (a chave deles ainda existe) e os novos entram
//! ao lado. Não é defeito: é o que "idempotente por chave" significa quando a
//! chave sorteada muda. Some sozinho na virada da janela.

use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

/// Com que frequência a janela vira. **Escolhida pela pessoa**, entre estas.
///
/// Três, e não cinco: a diferença entre "a cada 4 dias" e "a cada 5 dias" não é
/// uma escolha, é um número. Estas três são ritmos distintos — todo dia, de vez
/// em quando, toda semana.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cadencia {
    Diaria,
    TresDias,
    Semanal,
}

impl Cadencia {
    pub fn de(s: &str) -> Self {
        match s {
            "diaria" => Cadencia::Diaria,
            "tres_dias" => Cadencia::TresDias,
            _ => Cadencia::Semanal,
        }
    }

    pub fn chave(self) -> &'static str {
        match self {
            Cadencia::Diaria => "diaria",
            Cadencia::TresDias => "tres_dias",
            Cadencia::Semanal => "semanal",
        }
    }

    fn dias(self) -> i64 {
        match self {
            Cadencia::Diaria => 1,
            Cadencia::TresDias => 3,
            Cadencia::Semanal => 7,
        }
    }

    /// A janela que contém este instante.
    ///
    /// **Ancorada na segunda-feira local**, como a vitrine (§36) e o guia (§50),
    /// e pelo mesmo `deslocamento()` da emissora (§25). Ancorar em "sete dias a
    /// partir de quando você escolheu a cadência" faria a janela de cada pessoa
    /// flutuar, e trocar de cadência no meio da semana daria uma janela de meio
    /// dia sem que ninguém entendesse por quê.
    pub fn janela(
        self,
        agora: chrono::DateTime<chrono::Utc>,
    ) -> (chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>) {
        use chrono::{Datelike, NaiveTime, TimeZone};

        let desl = crate::live::emissora::deslocamento();
        let local = agora + desl;
        let hoje = local.date_naive();
        let segunda = hoje - chrono::Duration::days(hoje.weekday().num_days_from_monday() as i64);

        // Quantas janelas cheias couberam desde a segunda. Pra a semanal isso é
        // sempre zero; pra a de três dias, 0, 1 ou 2.
        let passados = (hoje - segunda).num_days();
        let inicio_local = segunda + chrono::Duration::days((passados / self.dias()) * self.dias());
        let fim_local = inicio_local + chrono::Duration::days(self.dias());

        let em_utc = |d: chrono::NaiveDate| {
            chrono::Utc.from_utc_datetime(&d.and_time(NaiveTime::MIN)) - desl
        };
        (em_utc(inicio_local), em_utc(fim_local))
    }
}

/// Que tipo de coisa o desafio pede, e como ele é conferido.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prova {
    /// Terminar qualquer obra.
    Terminar,
    /// Terminar uma obra com esta etiqueta (`genre`, `country`).
    TerminarComTag(&'static str),
    /// Terminar uma obra desta década.
    TerminarDaDecada,
    /// Terminar uma fita (§35: ano ≤ 1996).
    TerminarFita,
    /// Terminar uma obra deste diretor.
    TerminarDoDiretor,
    /// Avaliar qualquer obra.
    Avaliar,
    /// Escrever uma resenha (avaliação com texto).
    Resenhar,
    /// Pegar uma caixa emprestada.
    Alugar,
    /// Rebobinar a fita de outra pessoa.
    Rebobinar,
}

/// Uma definição de desafio.
struct Def {
    chave: &'static str,
    /// A frase, com `{}` onde o alvo entra.
    rotulo: &'static str,
    xp: i32,
    prova: Prova,
    /// De onde o alvo é sorteado. `None` = sem alvo.
    ///
    /// A consulta recebe `$1` = a pessoa e devolve **um** valor. As de
    /// "empurrão" excluem o que ela já viu — é o que faz o desafio empurrar em
    /// vez de repetir.
    alvo_sql: Option<&'static str>,
}

/// O `terminadas` de sempre — o §8f, a mesma definição que a curadoria, o guia,
/// a locadora, o mural e as conquistas usam. Uma sétima seria uma sétima chance
/// de discordarem.
const TERMINADAS: &str = r#"
    SELECT pe.work_id FROM play_event pe WHERE pe.user_id = $1
    GROUP BY pe.work_id
    HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
        OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
"#;

/// Os fáceis. Dopamina, e **não é só assistir**: avaliar, alugar e escrever
/// entram aqui porque o produto tem mais verbos que "dar play".
const FACEIS: &[Def] = &[
    Def { chave: "f_terminar", rotulo: "Termine qualquer obra", xp: 15, prova: Prova::Terminar, alvo_sql: None },
    Def { chave: "f_avaliar", rotulo: "Dê nota a uma obra", xp: 15, prova: Prova::Avaliar, alvo_sql: None },
    Def { chave: "f_alugar", rotulo: "Pegue uma caixa na locadora", xp: 15, prova: Prova::Alugar, alvo_sql: None },
    Def { chave: "f_resenhar", rotulo: "Escreva uma resenha", xp: 20, prova: Prova::Resenhar, alvo_sql: None },
    Def { chave: "f_rebobinar", rotulo: "Rebobine a fita de alguém", xp: 20, prova: Prova::Rebobinar, alvo_sql: None },
];

/// Os de tema. Específicos do acervo, e sorteados entre o que ele tem.
const TEMAS: &[Def] = &[
    Def {
        chave: "t_genero",
        rotulo: "Termine um filme de {}",
        xp: 30,
        prova: Prova::TerminarComTag("genre"),
        alvo_sql: Some(
            "SELECT t.value FROM tag t JOIN work_tag wt ON wt.tag_id = t.id
             WHERE t.namespace = 'genre'
             GROUP BY t.value HAVING count(*) >= 5
             ORDER BY md5($2 || t.value) LIMIT 1",
        ),
    },
    Def {
        chave: "t_decada",
        rotulo: "Termine algo dos anos {}",
        xp: 30,
        prova: Prova::TerminarDaDecada,
        alvo_sql: Some(
            "SELECT ((w.year / 10) * 10)::text FROM work w
             WHERE w.year IS NOT NULL AND w.kind = 'movie'
             GROUP BY 1 HAVING count(*) >= 5
             ORDER BY md5($2 || ((w.year / 10) * 10)::text) LIMIT 1",
        ),
    },
    Def { chave: "t_fita", rotulo: "Termine uma fita", xp: 30, prova: Prova::TerminarFita, alvo_sql: None },
];

/// Os de empurrão. **O que você nunca viu.**
///
/// É o único grupo que faz o desafio servir ao terceiro pilar (§1): sem ele, um
/// sistema de tarefas sorteadas do seu próprio gosto só reforça o gosto.
const EMPURROES: &[Def] = &[
    Def {
        chave: "e_pais",
        rotulo: "Termine algo de: {}",
        xp: 50,
        prova: Prova::TerminarComTag("country"),
        alvo_sql: Some(
            "SELECT t.value FROM tag t JOIN work_tag wt ON wt.tag_id = t.id
             WHERE t.namespace = 'country'
               AND NOT EXISTS (
                   SELECT 1 FROM work_tag wt2
                   WHERE wt2.tag_id = t.id AND wt2.work_id IN (TERMINADAS_AQUI))
             GROUP BY t.value HAVING count(*) >= 2
             ORDER BY md5($2 || t.value) LIMIT 1",
        ),
    },
    Def {
        chave: "e_genero",
        rotulo: "Termine um de {} — você nunca viu nenhum",
        xp: 50,
        prova: Prova::TerminarComTag("genre"),
        alvo_sql: Some(
            "SELECT t.value FROM tag t JOIN work_tag wt ON wt.tag_id = t.id
             WHERE t.namespace = 'genre'
               AND NOT EXISTS (
                   SELECT 1 FROM work_tag wt2
                   WHERE wt2.tag_id = t.id AND wt2.work_id IN (TERMINADAS_AQUI))
             GROUP BY t.value HAVING count(*) >= 3
             ORDER BY md5($2 || t.value) LIMIT 1",
        ),
    },
    Def {
        chave: "e_diretor",
        rotulo: "Termine algo de {}",
        xp: 50,
        prova: Prova::TerminarDoDiretor,
        alvo_sql: Some(
            "SELECT p.name FROM person p
             JOIN credit c ON c.person_id = p.id AND c.role = 'director'
             WHERE NOT EXISTS (
                 SELECT 1 FROM credit c2
                 WHERE c2.person_id = p.id AND c2.role = 'director'
                   AND c2.work_id IN (TERMINADAS_AQUI))
             GROUP BY p.name HAVING count(DISTINCT c.work_id) >= 3
             ORDER BY md5($2 || p.name) LIMIT 1",
        ),
    },
];

/// Um desafio, como a tela o vê.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DesafioNaTela {
    pub id: Uuid,
    pub chave: String,
    pub alvo: Option<String>,
    pub xp: i32,
    pub vence_em: chrono::DateTime<chrono::Utc>,
    pub cumprido_em: Option<chrono::DateTime<chrono::Utc>>,
    /// A frase pronta. Montada no servidor porque o `{}` do rótulo e o alvo
    /// moram aqui — mandar os dois separados faria a tela remontar a gramática.
    #[sqlx(skip)]
    pub rotulo: String,
}

/// Garante que a janela atual tem os três desafios, e devolve o que há.
///
/// **Idempotente**: o `UNIQUE (user_id, comeca_em, chave)` faz a segunda chamada
/// na mesma janela não inserir nada. Chamar isto a cada carregamento da tela é
/// barato e é o que dispensa um job de geração.
pub async fn da_janela(pool: &PgPool, quem: Uuid) -> Vec<DesafioNaTela> {
    let cadencia = cadencia_de(pool, quem).await;
    let (inicio, fim) = cadencia.janela(chrono::Utc::now());

    // A semente é **da pessoa, da janela e da cadência**: por pessoa porque o
    // desafio é individual (§2.4); pela cadência porque numa segunda-feira a
    // janela diária e a semanal começam no mesmo instante, e sem isso as duas
    // sorteariam o mesmo conjunto.
    let semente = format!("{quem}{inicio}{}", cadencia.chave());

    for grupo in [FACEIS, TEMAS, EMPURROES] {
        // Qual definição do grupo sai nesta janela — mesma semente, mais o
        // nome do grupo pra os três não caírem no mesmo índice.
        let i = (hash(&format!("{semente}{}", grupo[0].chave)) as usize) % grupo.len();
        let def = &grupo[i];

        let alvo = match def.alvo_sql {
            None => None,
            Some(sql) => {
                let sql = sql.replace("TERMINADAS_AQUI", TERMINADAS);
                match sqlx::query_scalar::<_, String>(&sql)
                    .bind(quem)
                    .bind(&semente)
                    .fetch_optional(pool)
                    .await
                {
                    Ok(a) => a,
                    Err(e) => {
                        tracing::warn!(erro = %e, chave = def.chave, "alvo do desafio falhou");
                        None
                    }
                }
            }
        };

        // Definição com alvo que não rendeu nada é **pulada**, não substituída
        // por um texto vazio. Acontece quando a pessoa já viu tudo daquele eixo
        // — e aí a janela vem com dois desafios, que é honesto: não há empurrão
        // possível. É o §24 aplicado a uma linha que não existe.
        if def.alvo_sql.is_some() && alvo.is_none() {
            continue;
        }

        let _ = sqlx::query(
            "INSERT INTO desafio (user_id, chave, alvo, xp, comeca_em, vence_em, cadencia)
             VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT DO NOTHING",
        )
        .bind(quem)
        .bind(def.chave)
        .bind(&alvo)
        .bind(def.xp)
        .bind(inicio)
        .bind(fim)
        .bind(cadencia.chave())
        .execute(pool)
        .await;
    }

    let mut lista = sqlx::query_as::<_, DesafioNaTela>(
        "SELECT id, chave, alvo, xp, vence_em, cumprido_em
         FROM desafio WHERE user_id = $1 AND comeca_em = $2 AND cadencia = $3
         ORDER BY xp",
    )
    .bind(quem)
    .bind(inicio)
    .bind(cadencia.chave())
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    for d in &mut lista {
        d.rotulo = frase(&d.chave, d.alvo.as_deref());
    }
    lista
}

/// A frase pronta.
fn frase(chave: &str, alvo: Option<&str>) -> String {
    let def = todas().into_iter().find(|d| d.chave == chave);
    let Some(def) = def else {
        return "desafio desconhecido".into();
    };
    match alvo {
        Some(a) => def.rotulo.replace("{}", a),
        None => def.rotulo.to_string(),
    }
}

fn todas() -> Vec<&'static Def> {
    FACEIS.iter().chain(TEMAS).chain(EMPURROES).collect()
}

/// Confere os desafios abertos e fecha os que foram cumpridos.
///
/// Chamada depois de gravar progresso, avaliação ou empréstimo. Uma consulta por
/// desafio aberto — no máximo três — e cada uma é um `EXISTS` sobre um índice
/// que já existe.
pub async fn conferir(pool: &PgPool, quem: Uuid) -> Vec<String> {
    let abertos: Vec<(Uuid, String, Option<String>, chrono::DateTime<chrono::Utc>, chrono::DateTime<chrono::Utc>)> =
        sqlx::query_as(
            "SELECT id, chave, alvo, comeca_em, vence_em FROM desafio
             WHERE user_id = $1 AND cumprido_em IS NULL AND vence_em > now()",
        )
        .bind(quem)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    let mut fechados = Vec::new();

    for (id, chave, alvo, inicio, fim) in abertos {
        let Some(def) = todas().into_iter().find(|d| d.chave == chave) else {
            continue;
        };

        let sql = prova_sql(def.prova);
        let feito: Option<Option<Uuid>> = sqlx::query_scalar(&sql)
            .bind(quem)
            .bind(inicio)
            .bind(fim)
            .bind(alvo.as_deref())
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();

        if let Some(work) = feito {
            let _ = sqlx::query(
                "UPDATE desafio SET cumprido_em = now(), cumprido_work = $2
                 WHERE id = $1 AND cumprido_em IS NULL",
            )
            .bind(id)
            .bind(work)
            .execute(pool)
            .await;
            fechados.push(frase(&chave, alvo.as_deref()));
        }
    }

    if !fechados.is_empty() {
        tracing::info!(quantos = fechados.len(), "desafios cumpridos");
    }
    fechados
}

/// A consulta que prova o cumprimento. Devolve a obra (ou `NULL`) quando houve.
///
/// **Tudo dentro da janela.** `$2` e `$3` são o começo e o fim, e é isso que faz
/// o desafio ser uma tarefa com prazo em vez de um fato acumulado: terminar um
/// filme de terror ontem não fecha o desafio de hoje.
fn prova_sql(p: Prova) -> String {
    // O "terminou nesta janela": o evento `finish` ou a passagem dos 92% (§8f)
    // com o carimbo dentro do prazo.
    const TERMINOU_NA_JANELA: &str = r#"
        SELECT pe.work_id FROM play_event pe
        WHERE pe.user_id = $1 AND pe.created_at >= $2 AND pe.created_at < $3
        GROUP BY pe.work_id
        HAVING count(*) FILTER (WHERE pe.event_type = 'finish') > 0
            OR max(pe.position_seconds / NULLIF(pe.duration_seconds, 0)) >= 0.92
    "#;

    match p {
        Prova::Terminar => format!("SELECT work_id FROM ({TERMINOU_NA_JANELA}) t LIMIT 1"),
        Prova::TerminarComTag(ns) => format!(
            "SELECT t.work_id FROM ({TERMINOU_NA_JANELA}) t
             JOIN work_tag wt ON wt.work_id = t.work_id
             JOIN tag g ON g.id = wt.tag_id AND g.namespace = '{ns}' AND g.value = $4
             LIMIT 1"
        ),
        Prova::TerminarDaDecada => format!(
            "SELECT t.work_id FROM ({TERMINOU_NA_JANELA}) t
             JOIN work w ON w.id = t.work_id
             WHERE (w.year / 10) * 10 = $4::int LIMIT 1"
        ),
        Prova::TerminarFita => format!(
            "SELECT t.work_id FROM ({TERMINOU_NA_JANELA}) t
             JOIN work w ON w.id = t.work_id
             WHERE w.year IS NOT NULL AND w.year <= {vhs} AND $4::text IS NOT DISTINCT FROM NULL
             LIMIT 1",
            vhs = crate::routes::locadora::ULTIMO_ANO_VHS
        ),
        Prova::TerminarDoDiretor => format!(
            "SELECT t.work_id FROM ({TERMINOU_NA_JANELA}) t
             JOIN credit c ON c.work_id = t.work_id AND c.role = 'director'
             JOIN person p ON p.id = c.person_id AND p.name = $4
             LIMIT 1"
        ),
        // Os que não são sobre assistir devolvem `NULL` como obra — a coluna
        // `cumprido_work` é anulável exatamente por isto.
        Prova::Avaliar => "SELECT NULL::uuid FROM avaliacao
             WHERE user_id = $1 AND atualizado_em >= $2 AND atualizado_em < $3
               AND $4::text IS NOT DISTINCT FROM NULL LIMIT 1"
            .into(),
        Prova::Resenhar => "SELECT NULL::uuid FROM avaliacao
             WHERE user_id = $1 AND atualizado_em >= $2 AND atualizado_em < $3
               AND texto IS NOT NULL AND $4::text IS NOT DISTINCT FROM NULL LIMIT 1"
            .into(),
        Prova::Alugar => "SELECT NULL::uuid FROM emprestimo
             WHERE user_id = $1 AND pego_em >= $2 AND pego_em < $3
               AND $4::text IS NOT DISTINCT FROM NULL LIMIT 1"
            .into(),
        Prova::Rebobinar => "SELECT NULL::uuid FROM rebobinada
             WHERE por = $1 AND de IS DISTINCT FROM $1
               AND quando >= $2 AND quando < $3
               AND $4::text IS NOT DISTINCT FROM NULL LIMIT 1"
            .into(),
    }
}

pub async fn cadencia_de(pool: &PgPool, quem: Uuid) -> Cadencia {
    let c: Option<(String,)> = sqlx::query_as("SELECT cadencia FROM perfil WHERE user_id = $1")
        .bind(quem)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    Cadencia::de(c.as_ref().map_or("semanal", |c| c.0.as_str()))
}

/// Quanto XP os desafios cumpridos renderam. Somado da **linha**, não da
/// definição: mudar o valor de um desafio amanhã não reescreve o XP de ontem.
pub async fn xp_ganho(pool: &PgPool, quem: Uuid) -> i64 {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT sum(xp)::bigint FROM desafio WHERE user_id = $1 AND cumprido_em IS NOT NULL",
    )
    .bind(quem)
    .fetch_one(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(0)
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A janela é ancorada na segunda-feira local**, como a vitrine e o guia.
    /// Ancorar em "sete dias a partir de quando você escolheu a cadência" faria
    /// a janela de cada pessoa flutuar — e trocar de cadência no meio da semana
    /// daria uma janela de meio dia sem ninguém entender por quê.
    #[test]
    fn a_janela_e_ancorada_na_segunda() {
        use chrono::Datelike;
        let desl = crate::live::emissora::deslocamento();
        // 2026-08-05 é uma quarta-feira.
        let t: chrono::DateTime<chrono::Utc> = "2026-08-05T14:00:00Z".parse().unwrap();

        let (i, f) = Cadencia::Semanal.janela(t);
        assert_eq!((i + desl).date_naive().weekday(), chrono::Weekday::Mon);
        assert_eq!(f - i, chrono::Duration::days(7));
        assert!(i <= t && t < f, "a janela não contém o instante");

        // A de três dias começa na segunda e anda de três em três: quarta cai
        // na primeira fatia (segunda, terça, quarta).
        let (i3, f3) = Cadencia::TresDias.janela(t);
        assert_eq!(f3 - i3, chrono::Duration::days(3));
        assert!(i3 <= t && t < f3);
        assert_eq!((i3 + desl).date_naive().weekday(), chrono::Weekday::Mon);

        let (id, fd) = Cadencia::Diaria.janela(t);
        assert_eq!(fd - id, chrono::Duration::days(1));
        assert!(id <= t && t < fd);
    }

    /// Três grupos, três desafios, e cada um com um trabalho diferente. Se um
    /// grupo esvaziar, a janela passa a vir com dois — e o teste é o que impede
    /// isso de acontecer por descuido.
    #[test]
    fn os_tres_grupos_existem_e_escalam() {
        assert!(!FACEIS.is_empty() && !TEMAS.is_empty() && !EMPURROES.is_empty());
        let f = FACEIS.iter().map(|d| d.xp).max().unwrap();
        let t = TEMAS.iter().map(|d| d.xp).min().unwrap();
        let e = EMPURROES.iter().map(|d| d.xp).min().unwrap();
        assert!(t >= f, "tema tem que valer pelo menos o fácil");
        assert!(e > t, "o empurrão tem que valer mais que o tema");
    }

    /// **Nenhuma chave repetida** entre os três grupos: elas são a chave única
    /// da tabela junto com a janela, e duas iguais fariam o segundo grupo não
    /// conseguir inserir o seu.
    #[test]
    fn as_chaves_sao_unicas() {
        let mut vistas = std::collections::HashSet::new();
        for d in todas() {
            assert!(vistas.insert(d.chave), "chave repetida: {}", d.chave);
        }
    }

    /// Todo rótulo com alvo tem `{}`, e todo rótulo sem alvo não tem. Um `{}`
    /// órfão apareceria literalmente na tela de alguém.
    #[test]
    fn o_rotulo_combina_com_o_alvo() {
        for d in todas() {
            assert_eq!(
                d.rotulo.contains("{}"),
                d.alvo_sql.is_some(),
                "rótulo e alvo discordam em {}",
                d.chave
            );
        }
        assert_eq!(frase("t_genero", Some("Terror")), "Termine um filme de Terror");
        assert_eq!(frase("f_terminar", None), "Termine qualquer obra");
    }

    /// **A prova é sempre dentro da janela.** Sem os dois carimbos, o desafio
    /// deixa de ser uma tarefa com prazo e vira um fato acumulado — e "termine
    /// um de terror" seria cumprido por um filme visto no ano passado.
    #[test]
    fn a_prova_e_sempre_dentro_da_janela() {
        for d in todas() {
            let sql = prova_sql(d.prova);
            assert!(sql.contains("$2"), "{} não olha o início da janela", d.chave);
            assert!(sql.contains("$3"), "{} não olha o fim da janela", d.chave);
        }
    }

    /// A semente é **da pessoa, da janela e da cadência**: dois usuários na
    /// mesma semana recebem desafios diferentes (§2.4 — o desafio é
    /// individual), e a mesma pessoa recebe os mesmos ao recarregar.
    #[test]
    fn a_semente_e_por_pessoa_janela_e_cadencia() {
        let a = uuid::uuid!("11111111-1111-1111-1111-111111111111");
        let b = uuid::uuid!("22222222-2222-2222-2222-222222222222");
        let j1 = "2026-08-03T03:00:00Z";
        let j2 = "2026-08-10T03:00:00Z";
        assert_ne!(hash(&format!("{a}{j1}semanal")), hash(&format!("{b}{j1}semanal")));
        assert_ne!(hash(&format!("{a}{j1}semanal")), hash(&format!("{a}{j2}semanal")));
        assert_eq!(hash(&format!("{a}{j1}semanal")), hash(&format!("{a}{j1}semanal")));
        // **E pela cadência.** Numa segunda-feira a janela diária e a semanal
        // começam no mesmo instante; sem a cadência na semente, as duas
        // sorteariam o mesmo conjunto e trocar de cadência não mudaria nada.
        assert_ne!(hash(&format!("{a}{j1}semanal")), hash(&format!("{a}{j1}diaria")));
    }

    /// **Numa segunda-feira, as três cadências começam juntas.** É consequência
    /// de todas serem ancoradas na segunda (como a vitrine e o guia), e é por
    /// isso que a cadência entrou na chave da tabela e na semente — sem ela,
    /// trocar de cadência numa segunda não gerava desafio nenhum.
    #[test]
    fn na_segunda_as_janelas_comecam_juntas() {
        let segunda: chrono::DateTime<chrono::Utc> = "2026-08-03T14:00:00Z".parse().unwrap();
        let (i7, f7) = Cadencia::Semanal.janela(segunda);
        let (i1, f1) = Cadencia::Diaria.janela(segunda);
        assert_eq!(i7, i1, "a âncora deixou de ser comum");
        assert_ne!(f7, f1, "e os fins têm que ser diferentes");
    }
}
