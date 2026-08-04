//! R26 — quem pode assistir o quê.
//!
//! ## Um lugar só, e é o ponto do módulo
//!
//! Antes desta fase a resposta era "quem está autenticado", e ela morava
//! implícita no middleware. A auditoria mostrou o custo: `/api/stream/{id}`
//! devolvia **206 para qualquer arquivo** a qualquer conta.
//!
//! A regra agora tem dono, e é aqui. Toda rota que entrega **bytes de mídia** —
//! stream direto, sessão HLS, legenda, folha de sprites, quadros do menu de DVD
//! — passa por `pode_assistir`. Espalhar a checagem pelos handlers é como o
//! defeito nasce: seis lugares, e o sétimo esquece.
//!
//! ## A regra
//!
//! | papel | pode assistir |
//! |---|---|
//! | `admin`, `user` | **tudo** — o disco é deles |
//! | `guest` | só o que pegou emprestado |
//!
//! E a escassez **não entra nesta conta**. Isso é R56, e desfaz a R50.
//!
//! ## O que a R50 leu errado, e o que a R56 corrigiu
//!
//! O pedido era: *"Para dar play nos filmes é necessário pegar emprestado
//! (SOMENTE MODO LOCADORA)"*.
//!
//! `(SOMENTE MODO LOCADORA)` tem duas leituras, e a R50 escolheu a primeira:
//!
//! 1. **"só quando o modo locadora estiver ligado"** — a escassez como chave, e
//!    a regra valendo em todo lugar que toca vídeo
//! 2. **"só dentro da locadora"** — a locadora como *lugar*, e a biblioteca
//!    fora dela
//!
//! Era a segunda. Nas palavras do dono, em 04/08/2026: *"a biblioteca é um modo
//! livre"*.
//!
//! ## O que isso muda no que a R50 argumentou
//!
//! A R50 dizia que uma regra com porta dos fundos não é regra — é tema. **Isso
//! continua verdade, e agora é o ponto:** a exigência de empréstimo vira
//! explicitamente uma regra *da locadora*, cumprida pela tela dela, e não um
//! cadeado sobre os bytes.
//!
//! E não podia ser diferente, por uma razão técnica que a R50 não enfrentou: o
//! servidor **não distingue** "abriu pela biblioteca" de "abriu pela locadora".
//! Quem pede é o mesmo `/plan` com o mesmo `media_file_id`. Uma regra que só
//! vale num lugar, num protocolo que não sabe de lugar, só pode ser cumprida por
//! quem sabe onde está — o cliente.
//!
//! Fazer o cliente **declarar** de onde veio seria pior: um cadeado cuja chave
//! está com quem ele deveria trancar. Melhor não fingir cadeado.
//!
//! ## A locadora não perdeu nada, e isso foi medido
//!
//! A tela da locadora **nunca consultou** esta regra. Ela decide com `comigo` —
//! se a caixa está na sua mão —, que é estado da própria caixa, não do acesso.
//! Os botões continuam dizendo "pegue emprestado", a caixa continua saindo da
//! prateleira, o prazo continua vencendo. A brincadeira está inteira.
//!
//! Quem consultava era a **biblioteca**: as coleções, a ficha, o "para você" e o
//! funil de dar play. Eram esses os afetados, e é deles que a regra saiu.
//!
//! ## O `guest` não muda, e é o que sobra de cadeado
//!
//! Pra ele o empréstimo sempre foi obrigatório — desde a R26, antes da escassez
//! existir. Ele não é dono do disco, e a biblioteca não é dele: o que ele
//! alcança é o que alguém lhe emprestou.
//!
//! Ou seja, esta função volta a ser exatamente o que era antes da R50.
//!
//! **A R28 tirou o círculo daqui, e a regra não mudou.** As duas consultas
//! abaixo cruzavam com `circulo_membro` pra confirmar que o convidado era do
//! grupo em que a fita foi pega. Com uma loja só, isso deixou de perguntar
//! alguma coisa: o empréstimo já é dele — `e.user_id = $1` — e um empréstimo de
//! outro grupo não existe mais. O que autoriza continua sendo o que sempre
//! autorizou, e agora está sozinho.
//!
//! A segunda linha não inventou nada: ela é a R19 (§35) deixando de valer
//! apenas para o morador. Uma cópia por caixa vira verdade técnica, o prazo
//! vira o fim do acesso, e a devolução automática vira a revogação.
//!
//! ## O que ela deliberadamente NÃO restringe
//!
//! **Navegar.** Um convidado lê o acervo inteiro: título, sinopse, elenco,
//! pôster. Uma locadora deixa ler a caixa toda antes de alugar, e um catálogo
//! que esconde o que existe não é uma loja — é um cofre com vitrine.
//!
//! Isso é escolha, não descuido: convidar alguém é dizer a essa pessoa o que
//! você tem. Quem não quiser dizer, não convida.

use sqlx::PgPool;
use uuid::Uuid;

use super::User;

/// Papéis que são donos do disco.
pub fn e_morador(user: &User) -> bool {
    matches!(user.role.as_str(), "admin" | "user")
}

/// Este usuário pode receber os bytes deste arquivo?
///
/// O `media_file` chega até a caixa pela obra, e a caixa pode ser a obra avulsa
/// **ou** a coleção da série — o mesmo par de colunas do `emprestimo` (§35).
/// O alcance de coleção é de dois níveis (série → temporada → obra), como o
/// `OBRAS_DA_CAIXA` da locadora, e pela mesma razão: a profundidade é conhecida.
pub async fn pode_assistir(pool: &PgPool, user: &User, media_file_id: Uuid) -> bool {
    // **R56: o morador passa, e a escassez não é consultada.**
    //
    // Era `($3 AND COALESCE((SELECT NOT escassez FROM locadora_opcoes), true))`,
    // e a subconsulta saiu junto com a regra — ver o cabeçalho do módulo.
    //
    // Como efeito colateral, esta função voltou a não tocar `locadora_opcoes`.
    // Ela roda a cada requisição de faixa do `<video>`, dezenas por minuto num
    // filme sendo assistido; é uma tabela a menos no caminho mais quente do
    // servidor.
    //
    // **`devolvido_em IS NULL` é a autorização inteira** — pro `guest`, que é
    // quem ainda depende dela. Quando a fita volta, por devolução ou por prazo
    // (§35), o acesso acaba no mesmo instante, sem nenhuma revogação em separado
    // pra alguém esquecer de escrever.
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT
        $3
        OR EXISTS (
            SELECT 1
            FROM media_file mf
            JOIN emprestimo e ON e.devolvido_em IS NULL AND e.user_id = $1
            WHERE mf.id = $2
              AND (
                  e.work_id = mf.work_id
                  OR (e.collection_id IS NOT NULL AND mf.work_id IN (
                        SELECT ci.work_id
                        FROM collection_item ci
                        JOIN collection c ON c.id = ci.collection_id
                        WHERE c.id = e.collection_id OR c.parent_id = e.collection_id
                  ))
              )
        )
        "#,
    )
    .bind(user.id)
    .bind(media_file_id)
    .bind(e_morador(user))
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// A mesma pergunta, quando o que se tem é a obra e não o arquivo — o menu de
/// DVD (§37) e as cenas trabalham assim.
pub async fn pode_assistir_obra(pool: &PgPool, user: &User, work_id: Uuid) -> bool {
    // R56, igual à irmã acima: o morador passa, a escassez não é consultada.
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT
        $3
        OR EXISTS (
            SELECT 1
            FROM emprestimo e
            WHERE e.devolvido_em IS NULL AND e.user_id = $1
              AND (
                  e.work_id = $2
                  OR (e.collection_id IS NOT NULL AND $2 IN (
                        SELECT ci.work_id
                        FROM collection_item ci
                        JOIN collection c ON c.id = ci.collection_id
                        WHERE c.id = e.collection_id OR c.parent_id = e.collection_id
                  ))
              )
        )
        "#,
    )
    .bind(user.id)
    .bind(work_id)
    .bind(e_morador(user))
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// A escassez está ligada?
///
/// **R56: isto não decide mais quem assiste.** Ela responde sobre a *locadora* —
/// se as caixas são uma cópia só, se pegar tira da prateleira, se o prazo vence.
/// A biblioteca não pergunta.
///
/// Continua servindo à tela da locadora, que precisa saber antes de desenhar os
/// botões dela, e ao `guest`, pra quem o empréstimo segue obrigatório.
///
/// E continua sendo a única leitura de `locadora_opcoes` fora do caminho dos
/// bytes — depois da R56, o caminho dos bytes não a lê mais em lugar nenhum.
pub async fn exige_emprestimo(pool: &PgPool) -> bool {
    sqlx::query_scalar::<_, bool>("SELECT escassez FROM locadora_opcoes")
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
}

/// O erro, numa frase só, pra não haver duas redações do mesmo "não".
///
/// Ela diz **onde resolver**, e não só que não pode: um 403 que não aponta a
/// saída é a parede que o §35 recusou quando desenhou o "pedir de volta".
pub fn negado() -> crate::error::AppError {
    crate::error::AppError::Forbidden(
        "você precisa pegar esta caixa emprestada na locadora pra assistir".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn com_papel(role: &str) -> User {
        User {
            id: Uuid::nil(),
            username: "x".into(),
            display_name: "X".into(),
            role: role.into(),
            is_active: true,
            created_at: Utc::now(),
            last_login_at: None,
        }
    }

    /// Morador é dono do disco; convidado não é. Esta é a linha inteira da
    /// R26 — se ela inverter, o convidado vira morador em silêncio.
    #[test]
    fn so_admin_e_user_sao_moradores() {
        assert!(e_morador(&com_papel("admin")));
        assert!(e_morador(&com_papel("user")));
        assert!(!e_morador(&com_papel("guest")));
        // Papel desconhecido **não** é morador. O `CHECK` da 0025 já impede
        // que ele exista, mas errar pro lado fechado é o único erro aceitável
        // aqui.
        assert!(!e_morador(&com_papel("qualquer-coisa")));
        assert!(!e_morador(&com_papel("")));
    }

    /// **A R56 libertou a biblioteca, e não o convidado. Isto guarda isso.**
    ///
    /// `e_morador` virou o primeiro termo inteiro do `SELECT` — com `true` o
    /// morador passa direto, com `false` sobra o `EXISTS` do empréstimo.
    ///
    /// Ou seja, esta função sozinha decide quem tem a biblioteca livre. Se
    /// alguém fizer `guest` devolver `true`, o convidado deixa de precisar de
    /// empréstimo e passa a alcançar o acervo inteiro — em silêncio, e sem
    /// nenhuma outra linha mudar.
    #[test]
    fn a_biblioteca_livre_e_do_morador_e_nao_do_convidado() {
        assert!(!e_morador(&com_papel("guest")));
        assert!(e_morador(&com_papel("admin")));
        assert!(e_morador(&com_papel("user")));
    }

    /// O 403 continua existindo, e continua sendo do convidado.
    ///
    /// A R56 tirou o "não" do caminho do morador, não do produto: um `guest`
    /// que tenta assistir o que ninguém lhe emprestou ainda recebe `negado()`, e
    /// a frase ainda tem que apontar a locadora.
    #[test]
    fn o_convidado_ainda_pode_ouvir_nao() {
        assert!(!e_morador(&com_papel("guest")));
        let crate::error::AppError::Forbidden(msg) = negado() else {
            panic!("negado() deixou de ser 403");
        };
        assert!(!msg.is_empty());
    }

    /// O "não" aponta a saída. Um 403 mudo faria o convidado concluir que o
    /// servidor está quebrado — que é a parede que o §35 recusou.
    #[test]
    fn o_nao_diz_onde_resolver() {
        let crate::error::AppError::Forbidden(msg) = negado() else {
            panic!("negado() deixou de ser 403");
        };
        assert!(msg.contains("locadora"), "o 403 não aponta a saída: {msg}");
    }
}
