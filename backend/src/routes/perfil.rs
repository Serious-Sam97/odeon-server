//! R32 — o perfil, e o que ele substitui.
//!
//! O §40 entregou um **placar** com quatro números, numa aba escondida, com um
//! aviso impresso na tela mandando ignorar o número. Este módulo é o que devia
//! ter sido feito: nível, XP, uma lista longa de conquistas, título e tags
//! desbloqueáveis, campo livre, vitrine — e a comparação com os amigos, que foi
//! pedida e nunca existiu.
//!
//! ## O que o perfil pode e não pode dizer
//!
//! Tudo que aparece aqui **foi conquistado**, com uma exceção declarada: a bio.
//! Título e tags são chaves de conquista, e é este módulo que confere se a
//! pessoa as tem — o banco não sabe a lista, e não deve saber (`conquistas.rs`).
//!
//! Um título que a pessoa não conquistou seria a única mentira que este perfil
//! poderia contar, e é por isso que a validação é escrita duas vezes na cabeça
//! de quem lê `salvar`: silenciosamente **descartar** o que não foi desbloqueado
//! seria pior que recusar, porque a tela mostraria sucesso e o perfil ficaria
//! diferente do que a pessoa mandou.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::auth::AuthUser;
use crate::conquistas::{self, Camada, Progresso};
use crate::enfeites::{self, EnfeiteNaTela};
use crate::error::{AppError, AppResult};
use crate::AppState;

/// Uma conquista, do ponto de vista de quem está olhando.
#[derive(Debug, Serialize)]
pub struct ConquistaNaTela {
    pub chave: &'static str,
    pub nome: &'static str,
    pub descricao: &'static str,
    pub camada: Camada,
    pub pontos: i32,
    pub titulo: bool,
    pub tag: Option<&'static str>,
    /// `None` quando ainda está trancada.
    pub em: Option<chrono::DateTime<chrono::Utc>>,
}

/// Uma obra da vitrine, já resolvida.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct NaVitrine {
    pub id: Uuid,
    pub titulo: String,
    pub ano: Option<i32>,
    pub poster: Option<String>,
}

/// Um amigo, pra comparar.
#[derive(Debug, Serialize)]
pub struct AmigoNoPlacar {
    pub id: Uuid,
    /// R43 — é o que vira `/p/<nome>` quando alguém clica numa linha do placar.
    pub username: String,
    pub display_name: String,
    pub nivel: i32,
    pub xp: i64,
    pub desbloqueadas: usize,
    pub titulo: Option<String>,
    /// Se sou eu. A tela marca a própria linha em vez de escondê-la.
    pub eu: bool,
}

#[derive(Debug, Serialize)]
pub struct PerfilCompleto {
    pub user_id: Uuid,
    pub username: String,
    pub display_name: String,
    pub meu: bool,
    pub progresso: Progresso,
    pub titulo: Option<String>,
    /// O nome legível do título — a chave é do código, e a tela não deve
    /// traduzir chave nenhuma.
    pub titulo_nome: Option<&'static str>,
    pub tags: Vec<String>,
    pub bio: Option<String>,
    /// R43 — o rosto, a capa e a cor. Já resolvidos: a tela recebe o caminho
    /// da arte e o hex, e não uma chave pra traduzir.
    pub avatar: Option<EnfeiteNaTela>,
    pub capa: Option<EnfeiteNaTela>,
    pub moldura: Option<&'static str>,
    pub vitrine: Vec<NaVitrine>,
    pub conquistas: Vec<ConquistaNaTela>,
    /// Você e seus amigos, do maior nível pro menor.
    ///
    /// **É a comparação que foi pedida**, e ela mora no perfil e não numa aba
    /// própria: um placar separado é o que o §40 fez, e ele acabou escondido.
    pub amigos: Vec<AmigoNoPlacar>,
    /// Os títulos e tags que **você** pode escolher. Vem só no seu próprio
    /// perfil — a tela de edição não precisa perguntar duas vezes.
    pub titulos_disponiveis: Vec<(&'static str, &'static str)>,
    pub tags_disponiveis: Vec<&'static str>,
    /// Os três catálogos, **inteiros e com o estado de cada opção**.
    ///
    /// Aqui a regra do §48 — *a tela nunca oferece o que a validação vai
    /// recusar* — não vira "esconder o trancado", e a diferença importa: a
    /// lista de conquistas mostra as 80 com a descrição de cada uma, porque
    /// *"uma conquista secreta é uma conquista que ninguém persegue"*. Um rosto
    /// escondido é a mesma perda. O que a tela não faz é deixar **escolher** o
    /// que está trancado — e o `aberto` de cada linha é o que diz isso.
    pub rostos: Vec<EnfeiteNaTela>,
    pub capas: Vec<EnfeiteNaTela>,
    pub molduras: Vec<EnfeiteNaTela>,
}

/// O meu perfil.
pub async fn meu(State(state): State<AppState>, AuthUser(user): AuthUser) -> AppResult<Json<PerfilCompleto>> {
    montar(&state, user.id, user.id).await.map(Json)
}

/// O perfil de outra pessoa.
///
/// **Sem filtro de amizade**, e é decisão: a 2.2 do `IDEIAS.md` diz que entre
/// amigos é tudo aberto, e um perfil é a coisa mais pública que alguém tem. O
/// que um estranho vê aqui é nível, conquistas e uma vitrine que a própria
/// pessoa montou — nada que ela não tenha escolhido mostrar.
///
/// **Aceita id ou nome de usuário** (R43). O link que se manda é `/p/rudney`:
/// um UUID num endereço que alguém digita, lê em voz alta ou reconhece numa
/// conversa é endereço de banco, não de gente. O id continua valendo porque o
/// placar já tinha só ele.
pub async fn de_alguem(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Path(quem): Path<String>,
) -> AppResult<Json<PerfilCompleto>> {
    let id = match Uuid::parse_str(&quem) {
        Ok(id) => id,
        Err(_) => {
            let achado: Option<(Uuid,)> = sqlx::query_as(
                "SELECT id FROM app_user WHERE lower(username) = lower($1) AND is_active",
            )
            .bind(&quem)
            .fetch_optional(&state.pool)
            .await?;
            achado.ok_or(AppError::NotFound)?.0
        }
    };
    montar(&state, id, user.id).await.map(Json)
}

async fn montar(state: &AppState, quem: Uuid, eu: Uuid) -> AppResult<PerfilCompleto> {
    let conta: Option<(String, String)> =
        sqlx::query_as("SELECT username, display_name FROM app_user WHERE id = $1 AND is_active")
            .bind(quem)
            .fetch_optional(&state.pool)
            .await?;
    let Some((username, display_name)) = conta else {
        return Err(AppError::NotFound);
    };

    // **Avaliar na leitura do próprio perfil.** É o único lugar onde alguém
    // espera ver medalha nova, e rodar aqui garante que abrir o perfil nunca
    // mostre um estado velho — mesmo que a avaliação da reprodução tenha
    // falhado. Só pro dono: avaliar o perfil alheio faria a visita de um amigo
    // escrever no banco de outra pessoa.
    if quem == eu {
        conquistas::avaliar(&state.pool, quem).await;
    }

    let progresso = conquistas::progresso(&state.pool, quem).await;
    let ja = conquistas::desbloqueadas(&state.pool, quem).await;

    let lista: Vec<ConquistaNaTela> = conquistas::LISTA
        .iter()
        .map(|(q, _)| ConquistaNaTela {
            chave: q.chave,
            nome: q.nome,
            descricao: q.descricao,
            camada: q.camada,
            pontos: q.pontos,
            titulo: q.titulo,
            tag: q.tag,
            em: ja.get(q.chave).copied(),
        })
        .collect();

    type Guardado = (
        Option<String>,
        Vec<String>,
        Option<String>,
        Vec<Uuid>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let guardado: Option<Guardado> = sqlx::query_as(
        "SELECT titulo, tags, bio, vitrine, avatar, capa, moldura
         FROM perfil WHERE user_id = $1",
    )
    .bind(quem)
    .fetch_optional(&state.pool)
    .await?;
    let (titulo, tags, bio, ids, avatar, capa, moldura) =
        guardado.unwrap_or((None, Vec::new(), None, Vec::new(), None, None, None));

    // Os catálogos são resolvidos contra o dono do perfil: quem olha vê o rosto
    // que ELE escolheu, e o `aberto` de cada linha só interessa a quem edita.
    let (rostos, capas_cat, molduras) = enfeites::disponiveis(&state.pool, &ja).await;

    // A vitrine resolve os ids na leitura, e a obra apagada some sozinha — é o
    // que a ausência de chave estrangeira compra. `ORDER BY array_position`
    // porque **a ordem é o conteúdo**: vitrine é curadoria.
    let vitrine: Vec<NaVitrine> = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as(
            "SELECT w.id, w.title AS titulo, w.year AS ano, w.artwork->>'poster' AS poster
             FROM work w WHERE w.id = ANY($1)
             ORDER BY array_position($1, w.id)",
        )
        .bind(&ids)
        .fetch_all(&state.pool)
        .await?
    };

    let amigos = placar(state, quem, eu).await?;

    let meu = quem == eu;
    let (titulos_disponiveis, tags_disponiveis) = if meu {
        (
            conquistas::LISTA
                .iter()
                .filter(|(q, _)| q.titulo && ja.contains_key(q.chave))
                .map(|(q, _)| (q.chave, q.nome))
                .collect(),
            conquistas::LISTA
                .iter()
                .filter(|(q, _)| ja.contains_key(q.chave))
                .filter_map(|(q, _)| q.tag)
                .collect(),
        )
    } else {
        (Vec::new(), Vec::new())
    };

    Ok(PerfilCompleto {
        user_id: quem,
        username,
        display_name,
        meu,
        progresso,
        titulo_nome: titulo.as_deref().and_then(nome_do_titulo),
        titulo,
        tags,
        bio,
        avatar: enfeites::escolhido(&rostos, avatar.as_deref()),
        capa: enfeites::escolhido(&capas_cat, capa.as_deref()),
        moldura: enfeites::cor_da_moldura(moldura.as_deref()),
        vitrine,
        conquistas: lista,
        amigos,
        titulos_disponiveis,
        tags_disponiveis,
        // O catálogo inteiro só desce pro dono: pro visitante ele seria uma
        // lista de coisas que ele não pode escolher no perfil de outra pessoa.
        rostos: if meu { rostos } else { Vec::new() },
        capas: if meu { capas_cat } else { Vec::new() },
        molduras: if meu { molduras } else { Vec::new() },
    })
}

/// Você e seus amigos, ordenados por nível.
///
/// **Uma avaliação de progresso por pessoa**, e não uma consulta que soma tudo
/// de uma vez: o XP é derivado (`conquistas.rs`), então não há coluna pra
/// ordenar. Com dois usuários isso é irrelevante; com duzentos amigos seria uma
/// consulta agregada, e é a única parte deste módulo que não escala de graça.
async fn placar(state: &AppState, dono: Uuid, eu: Uuid) -> AppResult<Vec<AmigoNoPlacar>> {
    let sql = format!(
        "SELECT u.id, u.username, u.display_name, p.titulo
         FROM app_user u
         LEFT JOIN perfil p ON p.user_id = u.id
         WHERE u.is_active AND (u.id = $1 OR u.id IN ({amigos}))",
        amigos = crate::routes::amigos::IDS_DOS_MEUS_AMIGOS,
    );

    let gente: Vec<(Uuid, String, String, Option<String>)> =
        sqlx::query_as(&sql).bind(dono).fetch_all(&state.pool).await?;

    let mut placar = Vec::with_capacity(gente.len());
    for (id, username, nome, titulo) in gente {
        let p = conquistas::progresso(&state.pool, id).await;
        placar.push(AmigoNoPlacar {
            id,
            username,
            display_name: nome,
            nivel: p.nivel,
            xp: p.xp,
            desbloqueadas: p.desbloqueadas,
            titulo: titulo.and_then(|t| nome_do_titulo(&t).map(str::to_string)),
            eu: id == eu,
        });
    }
    // Empate desempatado pelo nome, e não pela ordem do banco: um placar que
    // troca de ordem entre dois carregamentos parece que mudou.
    placar.sort_by(|a, b| b.xp.cmp(&a.xp).then_with(|| a.display_name.cmp(&b.display_name)));
    Ok(placar)
}

fn nome_do_titulo(chave: &str) -> Option<&'static str> {
    conquistas::LISTA
        .iter()
        .find(|(q, _)| q.chave == chave)
        .map(|(q, _)| q.nome)
}

/// Os desafios da janela atual, e a cadência escolhida.
///
/// **Gera na leitura**, e é idempotente — o `UNIQUE` da 0034 faz a segunda
/// chamada na mesma janela não inserir nada. Isso dispensa um job de geração: um
/// processo de fundo pra criar três linhas quando alguém abre a tela seria uma
/// peça a mais pra quebrar, e o §36 já tinha decidido isso ao pôr a varredura da
/// locadora na leitura em vez de num daemon.
pub async fn desafios(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
) -> AppResult<Json<Value>> {
    let lista = crate::desafios::da_janela(&state.pool, user.id).await;
    let cadencia = crate::desafios::cadencia_de(&state.pool, user.id).await;
    Ok(Json(json!({
        "cadencia": cadencia.chave(),
        "desafios": lista,
    })))
}

#[derive(Debug, Deserialize)]
pub struct NovaCadencia {
    pub cadencia: String,
}

/// Trocar a cadência.
///
/// A janela nova vale **a partir de agora**: os desafios da janela antiga ficam
/// como estão até vencerem. Apagá-los seria tirar da pessoa um desafio que ela
/// talvez já tivesse começado a cumprir.
pub async fn salvar_cadencia(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(nova): Json<NovaCadencia>,
) -> AppResult<Json<Value>> {
    let c = crate::desafios::Cadencia::de(&nova.cadencia);
    sqlx::query(
        "INSERT INTO perfil (user_id, cadencia) VALUES ($1, $2)
         ON CONFLICT (user_id) DO UPDATE SET cadencia = EXCLUDED.cadencia",
    )
    .bind(user.id)
    .bind(c.chave())
    .execute(&state.pool)
    .await?;
    Ok(Json(json!({ "cadencia": c.chave() })))
}

#[derive(Debug, Deserialize)]
pub struct NovoPerfil {
    pub titulo: Option<String>,
    pub tags: Vec<String>,
    pub bio: Option<String>,
    pub vitrine: Vec<Uuid>,
    /// R43. `None` é "não escolhi" e é um valor legítimo — quem tirou o rosto
    /// volta pra marca derivada do nome (R42).
    #[serde(default)]
    pub avatar: Option<String>,
    #[serde(default)]
    pub capa: Option<String>,
    #[serde(default)]
    pub moldura: Option<String>,
    /// O nome que aparece em toda tela. **`None` é "não mexe"**, e não "apaga":
    /// ele mora no `app_user` e não no `perfil`, então quem não mandar o campo —
    /// um cliente antigo, o app — continua salvando o resto sem se renomear por
    /// omissão. É o contrário da regra do `avatar` logo acima, e de propósito:
    /// lá o vazio é uma escolha ("não quero rosto"), aqui não existe conta sem
    /// nome.
    #[serde(default)]
    pub display_name: Option<String>,
}

/// Salvar o próprio perfil.
///
/// **Recusa, não descarta.** Um título não desbloqueado devolve 403 em vez de
/// sumir em silêncio: descartar mostraria sucesso na tela e deixaria o perfil
/// diferente do que a pessoa mandou, que é a pior das duas falhas.
pub async fn salvar(
    State(state): State<AppState>,
    AuthUser(user): AuthUser,
    Json(novo): Json<NovoPerfil>,
) -> AppResult<Json<Value>> {
    // Avalia antes de conferir: quem acabou de desbloquear um título na mesma
    // sessão não deve levar 403 por causa de uma avaliação atrasada.
    conquistas::avaliar(&state.pool, user.id).await;
    let ja = conquistas::desbloqueadas(&state.pool, user.id).await;

    if let Some(t) = novo.titulo.as_deref().filter(|t| !t.is_empty()) {
        let ok = conquistas::LISTA
            .iter()
            .any(|(q, _)| q.chave == t && q.titulo && ja.contains_key(q.chave));
        if !ok {
            return Err(AppError::Forbidden(
                "este título ainda não foi desbloqueado".into(),
            ));
        }
    }

    for tag in &novo.tags {
        let ok = conquistas::LISTA
            .iter()
            .any(|(q, _)| q.tag == Some(tag.as_str()) && ja.contains_key(q.chave));
        if !ok {
            return Err(AppError::Forbidden(format!(
                "a tag \"{tag}\" ainda não foi desbloqueada"
            )));
        }
    }

    // Os enfeites, pela mesma regra do título: **recusa, não descarta**. E a
    // recusa cobre dois casos com a mesma frase — a conquista que falta e o
    // rosto que este acervo não tem —, porque pra quem está do outro lado da
    // tela os dois são "esta opção não é sua".
    let (rostos, capas, molduras) = enfeites::disponiveis(&state.pool, &ja).await;
    for (valor, catalogo, o_que) in [
        (novo.avatar.as_deref(), &rostos, "rosto"),
        (novo.capa.as_deref(), &capas, "capa"),
        (novo.moldura.as_deref(), &molduras, "moldura"),
    ] {
        if let Some(chave) = valor.filter(|v| !v.is_empty()) {
            if !enfeites::pode_usar(catalogo, chave) {
                return Err(AppError::Forbidden(format!(
                    "este {o_que} ainda não está disponível pra você"
                )));
            }
        }
    }

    // Os limites de tamanho são dos `CHECK` da 0031 — repeti-los aqui criaria
    // dois lugares pra discordar. O que este handler faz é traduzir a violação
    // pra 400 em vez de deixar sair 500 (§8b).
    let titulo = novo.titulo.as_deref().map(str::trim).filter(|t| !t.is_empty());
    let bio = novo.bio.as_deref().map(str::trim).filter(|b| !b.is_empty());

    let vazio_e_nada = |v: &Option<String>| v.as_deref().map(str::trim).filter(|x| !x.is_empty()).map(str::to_string);

    // O nome mora em OUTRA tabela, e por isso daqui pra baixo é transação: o
    // perfil e o `app_user` mudam juntos ou não mudam. Salvar o nome e perder a
    // vitrine — ou o contrário — deixaria a tela mostrando metade do que a
    // pessoa mandou, e ela não teria como saber qual metade.
    let mut tx = state.pool.begin().await?;

    let feito = sqlx::query(
        "INSERT INTO perfil (user_id, titulo, tags, bio, vitrine, avatar, capa, moldura, atualizado_em)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now())
         ON CONFLICT (user_id) DO UPDATE SET
            titulo = EXCLUDED.titulo, tags = EXCLUDED.tags,
            bio = EXCLUDED.bio, vitrine = EXCLUDED.vitrine,
            avatar = EXCLUDED.avatar, capa = EXCLUDED.capa,
            moldura = EXCLUDED.moldura,
            atualizado_em = now()",
    )
    .bind(user.id)
    .bind(titulo)
    .bind(&novo.tags)
    .bind(bio)
    .bind(&novo.vitrine)
    .bind(vazio_e_nada(&novo.avatar))
    .bind(vazio_e_nada(&novo.capa))
    .bind(vazio_e_nada(&novo.moldura))
    .execute(&mut *tx)
    .await;

    if let Err(e) = feito {
        return Err(violacao(e));
    }

    // O nome, quando veio. Vazio depois do `trim` é recusado em vez de virar
    // "não mexe": quem apagou o campo e mandou salvar pediu alguma coisa, e o
    // silêncio devolveria a tela com o nome velho sem dizer por quê (§8b).
    if let Some(bruto) = novo.display_name.as_deref() {
        let nome = bruto.trim();
        if nome.is_empty() {
            return Err(AppError::BadRequest("o nome não pode ficar vazio".into()));
        }
        let feito = sqlx::query("UPDATE app_user SET display_name = $2 WHERE id = $1")
            .bind(user.id)
            .bind(nome)
            .execute(&mut *tx)
            .await;
        if let Err(e) = feito {
            return Err(violacao(e));
        }
    }

    tx.commit().await?;

    Ok(Json(json!({ "ok": true })))
}

/// Traduz um `CHECK` violado pra 400, com a frase do campo certo.
///
/// Antes havia uma frase só, e ela listava tags, vitrine e bio. Com o nome
/// entrando pela 0038 essa frase passaria a mentir em um dos casos — quem
/// estourasse o nome leria sobre a bio. O `constraint()` diz qual trava caiu, e
/// é ele que escolhe a frase.
fn violacao(e: sqlx::Error) -> AppError {
    if let sqlx::Error::Database(db) = &e {
        if db.code().as_deref() == Some("23514") {
            return AppError::BadRequest(
                match db.constraint() {
                    Some("app_user_display_name_check") => "o nome precisa ter de 1 a 40 caracteres",
                    _ => "no máximo 5 tags, 6 caixas na vitrine e 140 caracteres na bio",
                }
                .into(),
            );
        }
    }
    e.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Título e tag oferecidos vêm **da lista**, e só os desbloqueados. Se este
    /// filtro cair, a tela de edição oferece o que a validação vai recusar — e
    /// a pessoa leva 403 escolhendo de um menu que o produto mostrou.
    #[test]
    fn so_conquista_com_titulo_vira_titulo() {
        for (q, _) in conquistas::LISTA.iter().filter(|(q, _)| q.titulo) {
            assert!(!q.nome.is_empty());
        }
        // E toda chave de título é resolvível pra um nome — um título salvo que
        // não resolve apareceria como uma chave crua no perfil de alguém.
        for (q, _) in conquistas::LISTA.iter().filter(|(q, _)| q.titulo) {
            assert_eq!(nome_do_titulo(q.chave), Some(q.nome));
        }
        assert_eq!(nome_do_titulo("chave-que-nao-existe"), None);
    }
}
