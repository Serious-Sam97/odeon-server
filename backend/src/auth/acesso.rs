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
//! | papel | o que assiste |
//! |---|---|
//! | `admin`, `user` | **tudo** — o disco é deles, e barrar o player transformaria um morador em porteiro do outro (§35) |
//! | `guest` | **só o que pegou emprestado**, e enquanto o empréstimo estiver em aberto |
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
    if e_morador(user) {
        return true;
    }

    // Convidado: precisa de um empréstimo **dele**, em aberto, que cubra este
    // arquivo.
    //
    // **`devolvido_em IS NULL` é a autorização inteira.** Quando a fita volta —
    // por devolução ou por prazo (§35) — o acesso acaba no mesmo instante, sem
    // nenhuma revogação em separado pra alguém esquecer de escrever.
    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
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
    .fetch_one(pool)
    .await
    .unwrap_or(false)
}

/// A mesma pergunta, quando o que se tem é a obra e não o arquivo — o menu de
/// DVD (§37) e as cenas trabalham assim.
pub async fn pode_assistir_obra(pool: &PgPool, user: &User, work_id: Uuid) -> bool {
    if e_morador(user) {
        return true;
    }

    sqlx::query_scalar::<_, bool>(
        r#"
        SELECT EXISTS (
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
    .fetch_one(pool)
    .await
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
