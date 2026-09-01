//! O gênero do AniList, dito em português.
//!
//! ### O defeito
//!
//! O painel de filtros listava o mesmo gênero duas vezes:
//!
//! ```text
//! Comédia 3228   ·   Comedy 43
//! Ação 288       ·   Action 43
//! Aventura 230   ·   Adventure 43
//! Ficção científica 160  ·  Sci-Fi 43
//! ```
//!
//! Quem filtrava por "Comédia" perdia 43 filmes; quem filtrava por "Comedy"
//! perdia 3.228. É o mesmo problema que a §20 já teve uma vez e que o
//! `regiao.rs` documenta do lado dos países: **dois vocabulários dentro de um
//! namespace só**.
//!
//! ### De onde vem
//!
//! Todas as 43 obras de cada tag em inglês são **as mesmas 43** — a temporada
//! `anilist:288`, sem id do TMDB. O AniList não tem catálogo traduzido: a API
//! devolve `genres` em inglês e ponto. O TMDB devolve em pt-BR quando se pede,
//! e é por isso que o resto do acervo está em português.
//!
//! Então a tradução é aqui, na borda onde o vocabulário do AniList entra —
//! não na tela, que teria de conhecer os dois, e não numa query, que teria de
//! desfazer o dano toda vez.
//!
//! ### O que esta tabela NÃO toca
//!
//! `Action & Adventure` (959 obras), `Sci-Fi & Fantasy` (1.240) e `Kids` (332)
//! **não são duplicatas**: são os gêneros de série do TMDB, que a própria
//! tradução pt-BR do TMDB entrega em inglês. `Action & Adventure` é um grupo
//! próprio e não é a soma de `Ação` com `Aventura` — casar por palavra solta o
//! quebraria, e é o erro que este arquivo existe pra não cometer.
//!
//! A garantia não é cuidado, é estrutura: a tabela é indexada pelo nome exato
//! de um gênero do AniList, e `Action & Adventure` não é um. Nada que o TMDB
//! mande pode casar por acidente.

/// Os 18 gêneros do AniList → o vocabulário pt-BR que o acervo já usa.
///
/// Onde o TMDB já tinha um nome, é o nome dele: `Sci-Fi` vira "Ficção
/// científica" e não "Sci-Fi", senão a fusão não acontece. Onde ele não tem
/// (`Sports`), o nome novo entra em português mesmo assim.
///
/// **Mecha, Ecchi e Mahou Shoujo ficam como estão de propósito.** Não são
/// inglês: são termos japoneses que circulam em português exatamente assim, e
/// "Garota Mágica" seria uma tradução que ninguém procura.
const GENEROS: &[(&str, &str)] = &[
    ("Action", "Ação"),
    ("Adventure", "Aventura"),
    ("Comedy", "Comédia"),
    ("Drama", "Drama"),
    ("Fantasy", "Fantasia"),
    ("Horror", "Terror"),
    ("Music", "Música"),
    ("Mystery", "Mistério"),
    ("Romance", "Romance"),
    ("Sci-Fi", "Ficção científica"),
    ("Sports", "Esporte"),
    ("Psychological", "Psicológico"),
    ("Slice of Life", "Cotidiano"),
    ("Supernatural", "Sobrenatural"),
    ("Thriller", "Thriller"),
    ("Ecchi", "Ecchi"),
    ("Mecha", "Mecha"),
    ("Mahou Shoujo", "Mahou Shoujo"),
];

/// O gênero em português, ou ele mesmo quando não está na tabela.
///
/// Cair nele mesmo é a regra do §18 de novo: um gênero novo do AniList aparece
/// em inglês na tela, visível, pedindo uma linha aqui — melhor que sumir.
pub fn em_portugues(genero: &str) -> String {
    GENEROS
        .iter()
        .find(|(ingles, _)| *ingles == genero)
        .map(|(_, portugues)| portugues.to_string())
        .unwrap_or_else(|| genero.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Os quatro pares que a tela mostrava lado a lado.
    #[test]
    fn as_duplicatas_medidas_caem_no_nome_que_ja_existe() {
        assert_eq!(em_portugues("Comedy"), "Comédia");
        assert_eq!(em_portugues("Action"), "Ação");
        assert_eq!(em_portugues("Adventure"), "Aventura");
        assert_eq!(em_portugues("Sci-Fi"), "Ficção científica");
    }

    /// O aviso do cliente, virado teste: `Action & Adventure` é grupo próprio,
    /// com 959 obras, e não é a soma de `Ação` com `Aventura`.
    #[test]
    fn os_generos_de_serie_do_tmdb_passam_intactos() {
        for genero in ["Action & Adventure", "Sci-Fi & Fantasy", "Kids", "War & Politics"] {
            assert_eq!(em_portugues(genero), genero, "{genero} foi mexido");
        }
    }

    /// O perigo é real e é por isso que o casamento é pelo nome **exato**:
    /// `Action` é prefixo de `Action & Adventure`, e `Fantasy` é sufixo de
    /// `Sci-Fi & Fantasy`. Uma regra por prefixo, sufixo ou palavra solta
    /// converteria 2.199 obras de dois grupos próprios em "Ação" e "Fantasia"
    /// — e ninguém notaria de imediato, porque o resultado parece plausível.
    #[test]
    fn o_casamento_por_pedaco_seria_o_estrago_e_por_isso_nao_existe() {
        assert!("Action & Adventure".starts_with("Action"));
        assert!("Sci-Fi & Fantasy".ends_with("Fantasy"));
        assert_eq!(em_portugues("Action & Adventure"), "Action & Adventure");
        assert_eq!(em_portugues("Sci-Fi & Fantasy"), "Sci-Fi & Fantasy");
    }

    /// Termo japonês não vira tradução inventada.
    #[test]
    fn emprestimo_do_japones_fica_como_esta() {
        assert_eq!(em_portugues("Mecha"), "Mecha");
        assert_eq!(em_portugues("Ecchi"), "Ecchi");
    }

    /// Gênero fora da tabela aparece cru — é um pedido de linha, não um erro.
    #[test]
    fn genero_novo_do_anilist_nao_some() {
        assert_eq!(em_portugues("Gourmet"), "Gourmet");
    }

    /// Nenhum alvo pode estar em inglês: a tabela existe pra acabar com o
    /// segundo vocabulário, não pra reorganizá-lo.
    #[test]
    fn nenhum_alvo_e_traducao_de_si_mesmo_em_ingles() {
        const EMPRESTIMOS: &[&str] = &["Thriller", "Drama", "Romance", "Ecchi", "Mecha", "Mahou Shoujo"];
        for (ingles, portugues) in GENEROS {
            assert!(
                ingles != portugues || EMPRESTIMOS.contains(ingles),
                "{ingles} continua em inglês sem estar na lista de empréstimos"
            );
        }
    }
}
