//! País e idioma — os códigos do provider, ditos em português.
//!
//! ### Por que isto é tag, e não coluna
//!
//! A tentação era `work.pais text`. Seria a saída preguiçosa e teria repetido o
//! erro que o §8b já registrou uma vez, quando `anime` quase virou um `kind`:
//! **uma obra tem várias facetas ao mesmo tempo**, e uma coprodução
//! França/Itália/Alemanha não cabe numa coluna.
//!
//! Como tag, tudo que o M2 construiu passa a valer de graça: o filtro composto
//! de `/api/works` (`tags=country:França`), a curadoria do M5, as estantes da
//! locadora e o eixo novo da wiki. Zero código de consulta.
//!
//! ### Por que traduzir
//!
//! O TMDB devolve `production_countries` **em inglês mesmo quando se pede
//! `pt-BR`** — verificado: *Pulp Fiction* volta com "United States of America"
//! ao lado de uma sinopse em português. Aceitar isso criaria, dentro do mesmo
//! namespace, o problema dos dois vocabulários que a §20 já teve com os gêneros
//! (pt-BR nos filmes, inglês nas séries) e que obrigou cada estante da locadora
//! a listar rótulos nas duas línguas.
//!
//! Mas junto do nome vem o **código ISO 3166-1**, e código não tem idioma. A
//! tradução aqui é uma tabela sobre o código — determinística, sem heurística e
//! sem chamada de rede.
//!
//! **Código desconhecido não é inventado.** Ele cai no nome que o provider
//! mandou, em inglês, e fica visível: um rótulo em inglês no meio da tela é um
//! pedido de acréscimo à tabela, e é preferível a um chute com cara de tradução
//! (§18).

/// ISO 3166-1 alfa-2 → nome em português.
///
/// A lista cobre os países que produzem cinema em volume; o que faltar aparece
/// com o nome do provider até alguém acrescentar aqui.
const PAISES: &[(&str, &str)] = &[
    ("US", "Estados Unidos"),
    ("GB", "Reino Unido"),
    ("FR", "França"),
    ("DE", "Alemanha"),
    ("IT", "Itália"),
    ("ES", "Espanha"),
    ("PT", "Portugal"),
    ("BR", "Brasil"),
    ("AR", "Argentina"),
    ("MX", "México"),
    ("CL", "Chile"),
    ("CO", "Colômbia"),
    ("UY", "Uruguai"),
    ("CA", "Canadá"),
    ("JP", "Japão"),
    ("CN", "China"),
    ("HK", "Hong Kong"),
    ("TW", "Taiwan"),
    ("KR", "Coreia do Sul"),
    ("IN", "Índia"),
    ("TH", "Tailândia"),
    ("AU", "Austrália"),
    ("NZ", "Nova Zelândia"),
    ("IE", "Irlanda"),
    ("BE", "Bélgica"),
    ("NL", "Países Baixos"),
    ("SE", "Suécia"),
    ("NO", "Noruega"),
    ("DK", "Dinamarca"),
    ("FI", "Finlândia"),
    ("IS", "Islândia"),
    ("PL", "Polônia"),
    ("CZ", "Tchéquia"),
    ("SK", "Eslováquia"),
    ("HU", "Hungria"),
    ("AT", "Áustria"),
    ("CH", "Suíça"),
    ("RU", "Rússia"),
    ("UA", "Ucrânia"),
    ("RO", "Romênia"),
    ("GR", "Grécia"),
    ("TR", "Turquia"),
    ("IL", "Israel"),
    ("IR", "Irã"),
    ("EG", "Egito"),
    ("ZA", "África do Sul"),
    ("NG", "Nigéria"),
    ("MA", "Marrocos"),
    ("LU", "Luxemburgo"),
    ("HR", "Croácia"),
    ("RS", "Sérvia"),
    ("BG", "Bulgária"),
    ("PH", "Filipinas"),
    ("ID", "Indonésia"),
    ("MY", "Malásia"),
    ("SG", "Singapura"),
    ("VN", "Vietnã"),
    ("PK", "Paquistão"),
    ("SU", "União Soviética"),
    ("CS", "Tchecoslováquia"),
    ("YU", "Iugoslávia"),
];

/// ISO 639-1 → nome do idioma em português.
const IDIOMAS: &[(&str, &str)] = &[
    ("en", "inglês"),
    ("pt", "português"),
    ("es", "espanhol"),
    ("fr", "francês"),
    ("de", "alemão"),
    ("it", "italiano"),
    ("ja", "japonês"),
    ("zh", "chinês"),
    ("cn", "chinês"),
    ("ko", "coreano"),
    ("ru", "russo"),
    ("hi", "híndi"),
    ("ta", "tâmil"),
    ("te", "télugo"),
    ("ar", "árabe"),
    ("he", "hebraico"),
    ("fa", "persa"),
    ("tr", "turco"),
    ("sv", "sueco"),
    ("no", "norueguês"),
    ("da", "dinamarquês"),
    ("fi", "finlandês"),
    ("is", "islandês"),
    ("nl", "neerlandês"),
    ("pl", "polonês"),
    ("cs", "tcheco"),
    ("sk", "eslovaco"),
    ("hu", "húngaro"),
    ("el", "grego"),
    ("ro", "romeno"),
    ("uk", "ucraniano"),
    ("th", "tailandês"),
    ("id", "indonésio"),
    ("vi", "vietnamita"),
    ("ms", "malaio"),
    ("tl", "filipino"),
    ("la", "latim"),
    ("xx", "sem diálogo"),
];

/// O nome do país em português, ou o que o provider mandou.
pub fn pais(iso: &str, nome_do_provider: &str) -> String {
    let chave = iso.trim().to_ascii_uppercase();
    PAISES
        .iter()
        .find(|(codigo, _)| *codigo == chave)
        .map(|(_, nome)| nome.to_string())
        .unwrap_or_else(|| nome_do_provider.trim().to_string())
}

/// O nome do idioma em português. `None` quando o código não é conhecido —
/// aqui **não há nome do provider pra cair**, e um código cru (`"sr"`) como
/// rótulo de tag seria pior que a ausência da tag.
pub fn idioma(iso: &str) -> Option<String> {
    let chave = iso.trim().to_ascii_lowercase();
    IDIOMAS
        .iter()
        .find(|(codigo, _)| *codigo == chave)
        .map(|(_, nome)| nome.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traduz_pelo_codigo() {
        assert_eq!(pais("US", "United States of America"), "Estados Unidos");
        assert_eq!(pais("br", "Brazil"), "Brasil");
    }

    #[test]
    fn codigo_desconhecido_cai_no_provider_em_vez_de_inventar() {
        assert_eq!(pais("ZZ", "Ruritânia"), "Ruritânia");
    }

    #[test]
    fn idioma_desconhecido_nao_vira_tag() {
        assert_eq!(idioma("ja").as_deref(), Some("japonês"));
        // Um código cru como rótulo seria ruído com cara de dado.
        assert_eq!(idioma("qq"), None);
    }

    /// Os países históricos importam num acervo de cinema: metade dos clássicos
    /// do leste europeu vem com `SU` ou `CS`.
    #[test]
    fn paises_que_nao_existem_mais_estao_na_tabela() {
        assert_eq!(pais("SU", "Soviet Union"), "União Soviética");
        assert_eq!(pais("YU", "Yugoslavia"), "Iugoslávia");
    }
}
