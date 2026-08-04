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
//! | papel | escassez desligada | escassez **ligada** |
//! |---|---|---|
//! | `admin`, `user` | **tudo** — o disco é deles | **só o que pegou emprestado** |
//! | `guest` | só o emprestado | só o emprestado |
//!
//! ## A chave é a escassez, e isso é R50
//!
//! *"Para dar play nos filmes é necessário pegar emprestado (SOMENTE MODO
//! LOCADORA)"* — e o "modo locadora" **já existia**: é a escassez da R29, que
//! significa *"uma cópia por caixa, e quem pegou tirou da prateleira"*.
//!
//! Exigir o empréstimo pra assistir é a **consequência** disso, não uma regra
//! ao lado. Com a escassez desligada a locadora é um tema; com ela ligada, é o
//! mecanismo — e uma cópia que some da estante mas continua tocando pra todo
//! mundo nunca foi uma cópia só.
//!
//! Por isso não há chave nova no painel: seria uma segunda chave dizendo a
//! mesma coisa, e duas chaves pra uma ideia é como um estado impossível nasce.
//!
//! ## Vale pro administrador também, e é decisão
//!
//! Uma regra com porta dos fundos pro dono não é uma regra — é um tema. O `admin`
//! entra na fila como todo mundo, e o que o distingue continua sendo o que sempre
//! distinguiu: ele **desliga a escassez** quando quiser, num clique, pra casa
//! inteira de uma vez.
//!
//! O que isto **não** muda: quem já é `guest` continua exatamente como estava,
//! porque pra ele o empréstimo sempre foi obrigatório.
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
    // **A escassez é lida na MESMA consulta**, e não numa antes.
    //
    // Esta função roda a cada requisição de faixa do `<video>` — dezenas por
    // minuto num filme sendo assistido. Ler a opção em separado dobraria as idas
    // ao banco de tudo que toca. Aqui é uma linha a mais num `SELECT` que já
    // existia, sobre uma tabela de **uma** linha com chave primária.
    //
    // O `COALESCE` erra pro lado aberto de propósito: sem linha de opções não há
    // locadora configurada, e trancar o disco da casa por causa de uma tabela
    // vazia seria transformar uma ausência de configuração em bloqueio.
    //
    // **`devolvido_em IS NULL` é a autorização inteira.** Quando a fita volta —
    // por devolução ou por prazo (§35) — o acesso acaba no mesmo instante, sem
    // nenhuma revogação em separado pra alguém esquecer de escrever.
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT
        ($3 AND COALESCE((SELECT NOT escassez FROM locadora_opcoes), true))
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
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT
        ($3 AND COALESCE((SELECT NOT escassez FROM locadora_opcoes), true))
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

/// A regra está valendo agora?
///
/// A tela precisa saber **antes** de desenhar o botão: um ▸ assistir que leva
/// 403 é o §8b, e o §53 já disse que o produto não oferece o que ele sabe que
/// vai negar. É a única leitura da opção feita fora do caminho dos bytes.
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

    /// **A R50 não mexeu no convidado, e isto guarda isso.**
    ///
    /// `e_morador` é o único parâmetro que a consulta recebe além do usuário e
    /// do alvo: com `false`, o primeiro termo do `SELECT` morre e sobra o
    /// `EXISTS` do empréstimo — exatamente a regra que o convidado já tinha
    /// desde a R26. Se alguém inverter esta função, o convidado vira morador em
    /// silêncio e a escassez deixa de valer pra ele.
    #[test]
    fn a_escassez_nao_afrouxa_o_convidado() {
        assert!(!e_morador(&com_papel("guest")));
        // E o administrador entra na conta como qualquer morador: é ele que
        // desliga a escassez, não que escapa dela.
        assert!(e_morador(&com_papel("admin")));
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
