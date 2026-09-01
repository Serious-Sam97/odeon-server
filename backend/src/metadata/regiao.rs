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

/// ISO 639-2 → ISO 639-1, pra a tabela de nomes ser uma só.
///
/// O TMDB fala alfa-2 (`pt`), mas o **arquivo** fala alfa-3: o `language` que o
/// ffprobe lê da tag do container é 639-2, e é ele que aparecia cru no seletor
/// de faixas (`por (5.1)`).
///
/// O 639-2 tem dois vocabulários para as mesmas línguas — o bibliográfico, do
/// nome em inglês (`ger`, `fre`, `chi`), e o terminológico, do nome nativo
/// (`deu`, `fra`, `zho`). **Os dois circulam neste acervo ao mesmo tempo**:
/// medido, 33 faixas `fre` e 29 `fra`, 27 `ger` e nenhuma `deu`. Aceitar só um
/// deixaria metade do francês sem nome, então ambos entram.
const ALFA3: &[(&str, &str)] = &[
    ("eng", "en"),
    ("por", "pt"),
    ("spa", "es"),
    ("fre", "fr"),
    ("fra", "fr"),
    ("ger", "de"),
    ("deu", "de"),
    ("ita", "it"),
    ("jpn", "ja"),
    ("chi", "zh"),
    ("zho", "zh"),
    ("kor", "ko"),
    ("rus", "ru"),
    ("hin", "hi"),
    ("tam", "ta"),
    ("tel", "te"),
    ("ara", "ar"),
    ("heb", "he"),
    ("per", "fa"),
    ("fas", "fa"),
    ("tur", "tr"),
    ("swe", "sv"),
    ("nor", "no"),
    ("nob", "no"),
    ("nno", "no"),
    ("dan", "da"),
    ("fin", "fi"),
    ("ice", "is"),
    ("isl", "is"),
    ("dut", "nl"),
    ("nld", "nl"),
    ("pol", "pl"),
    ("cze", "cs"),
    ("ces", "cs"),
    ("slo", "sk"),
    ("slk", "sk"),
    ("hun", "hu"),
    ("gre", "el"),
    ("ell", "el"),
    ("rum", "ro"),
    ("ron", "ro"),
    ("ukr", "uk"),
    ("tha", "th"),
    ("ind", "id"),
    ("vie", "vi"),
    ("may", "ms"),
    ("msa", "ms"),
    ("tgl", "tl"),
    ("fil", "tl"),
    ("lat", "la"),
    ("zxx", "xx"),
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

/// Países que **não** pedem "de" seco, e a contração que pedem.
///
/// Em português o país carrega artigo — *o* Canadá, *a* França, *os* Estados
/// Unidos — e ele contrai com a preposição. Quem não está aqui é dos que
/// dispensam artigo (Portugal, Israel, Cuba) e leva "de".
///
/// A tabela é indexada pelo **nome**, não pelo código, porque é o nome que a
/// tag guarda (`country:Canadá`) e é a tag que a curadoria lê. País fora da
/// tabela de `PAISES` chega aqui com o nome em inglês do provider e cai no
/// "de", que é o menos errado dos erros possíveis.
const ARTIGO_DO_PAIS: &[(&str, &str)] = &[
    ("Estados Unidos", "dos"),
    ("Países Baixos", "dos"),
    ("Filipinas", "das"),
    ("Reino Unido", "do"),
    ("Brasil", "do"),
    ("México", "do"),
    ("Chile", "do"),
    ("Uruguai", "do"),
    ("Canadá", "do"),
    ("Japão", "do"),
    ("Irã", "do"),
    ("Egito", "do"),
    ("Marrocos", "do"),
    ("Vietnã", "do"),
    ("Paquistão", "do"),
    ("França", "da"),
    ("Alemanha", "da"),
    ("Itália", "da"),
    ("Espanha", "da"),
    ("Argentina", "da"),
    ("Colômbia", "da"),
    ("China", "da"),
    ("Coreia do Sul", "da"),
    ("Índia", "da"),
    ("Tailândia", "da"),
    ("Austrália", "da"),
    ("Nova Zelândia", "da"),
    ("Irlanda", "da"),
    ("Bélgica", "da"),
    ("Suécia", "da"),
    ("Noruega", "da"),
    ("Dinamarca", "da"),
    ("Finlândia", "da"),
    ("Islândia", "da"),
    ("Polônia", "da"),
    ("Tchéquia", "da"),
    ("Eslováquia", "da"),
    ("Hungria", "da"),
    ("Áustria", "da"),
    ("Suíça", "da"),
    ("Rússia", "da"),
    ("Ucrânia", "da"),
    ("Romênia", "da"),
    ("Grécia", "da"),
    ("Turquia", "da"),
    ("África do Sul", "da"),
    ("Nigéria", "da"),
    ("Croácia", "da"),
    ("Sérvia", "da"),
    ("Bulgária", "da"),
    ("Indonésia", "da"),
    ("Malásia", "da"),
    ("União Soviética", "da"),
    ("Tchecoslováquia", "da"),
    ("Iugoslávia", "da"),
];

/// "do Canadá", "da França", "de Portugal" — o país pronto pra entrar numa
/// frase.
///
/// Existe aqui, e não em quem monta a frase, porque a contração é um fato
/// sobre o país, do mesmo tipo que o nome. Quem escreve a frase sabe que quer
/// dizer "de onde vem"; quem sabe que é *o* Canadá é esta tabela.
pub fn de_pais(nome: &str) -> String {
    let contracao = ARTIGO_DO_PAIS
        .iter()
        .find(|(pais, _)| *pais == nome)
        .map(|(_, artigo)| *artigo)
        .unwrap_or("de");
    format!("{contracao} {nome}")
}

/// O nome do país em português, ou o que o provider mandou.
pub fn pais(iso: &str, nome_do_provider: &str) -> String {
    let chave = iso.trim().to_ascii_uppercase();
    PAISES
        .iter()
        .find(|(codigo, _)| *codigo == chave)
        .map(|(_, nome)| nome.to_string())
        .unwrap_or_else(|| nome_do_provider.trim().to_string())
}

/// O nome do idioma em português, de um código alfa-2 **ou** alfa-3. `None`
/// quando o código não é conhecido — aqui **não há nome do provider pra cair**,
/// e um código cru (`"sr"`) como rótulo de tag seria pior que a ausência da tag.
pub fn idioma(iso: &str) -> Option<String> {
    let bruto = iso.trim().to_ascii_lowercase();
    // `pt-BR` e `pt_br` são a mesma língua que `pt`: a região é sufixo de
    // localidade, e ela aparece nos nomes de arquivo de legenda.
    let bruto = bruto.split(['-', '_']).next().unwrap_or(&bruto).to_string();
    let chave = ALFA3
        .iter()
        .find(|(alfa3, _)| *alfa3 == bruto)
        .map(|(_, alfa2)| *alfa2)
        .unwrap_or(&bruto);
    IDIOMAS
        .iter()
        .find(|(codigo, _)| *codigo == chave)
        .map(|(_, nome)| nome.to_string())
}

/// O mesmo nome, com inicial maiúscula.
///
/// Existe separado porque os dois usos querem caixas diferentes e nenhum dos
/// dois é o "certo": como **valor de tag** o idioma é minúsculo, igual aos 12
/// que já estão no banco (`lang:português`); como **abertura de rótulo** ele é
/// o começo de uma frase curta na tela (`Português (5.1)`). Capitalizar no
/// cliente devolveria a regra pros quatro aparelhos, que é justamente o que o
/// `label` pronto existe pra evitar.
pub fn idioma_capitalizado(iso: &str) -> Option<String> {
    idioma(iso).map(|nome| {
        let mut chars = nome.chars();
        match chars.next() {
            Some(primeira) => primeira.to_uppercase().collect::<String>() + chars.as_str(),
            None => nome,
        }
    })
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

    /// O código que vem do arquivo é alfa-3, e ele tem de cair na mesma tabela
    /// de nomes do alfa-2 do provider — senão o seletor de faixas mostra `por`.
    #[test]
    fn o_alfa3_do_arquivo_cai_na_tabela_do_alfa2() {
        assert_eq!(idioma("por").as_deref(), Some("português"));
        assert_eq!(idioma("eng").as_deref(), Some("inglês"));
        assert_eq!(idioma("jpn").as_deref(), Some("japonês"));
        // Alfa-2 continua funcionando: é o que o TMDB manda.
        assert_eq!(idioma("pt").as_deref(), Some("português"));
    }

    /// Os dois vocabulários do 639-2 convivem no acervo — 33 faixas `fre` e 29
    /// `fra`, 27 `ger` e nenhuma `deu`. Aceitar um só deixaria metade sem nome.
    #[test]
    fn bibliografico_e_terminologico_dao_no_mesmo_nome() {
        assert_eq!(idioma("fre"), idioma("fra"));
        assert_eq!(idioma("ger"), idioma("deu"));
        assert_eq!(idioma("fre").as_deref(), Some("francês"));
    }

    /// Alfa-3 desconhecido não vira nome inventado nem vira alfa-2 por corte.
    /// `byn` (blin) e `new` (newari) existem neste acervo, quase certamente por
    /// engano de quem rippou — e `new` cortado em `ne` viraria "nepali".
    #[test]
    fn alfa3_fora_da_tabela_nao_e_chutado() {
        assert_eq!(idioma("byn"), None);
        assert_eq!(idioma("new"), None);
    }

    #[test]
    fn o_rotulo_abre_em_maiuscula_e_a_tag_nao() {
        assert_eq!(idioma_capitalizado("por").as_deref(), Some("Português"));
        assert_eq!(idioma("por").as_deref(), Some("português"));
        // Acento na primeira letra não pode perder o resto da palavra.
        assert_eq!(idioma_capitalizado("ara").as_deref(), Some("Árabe"));
    }

    /// O sufixo de região vem dos nomes de arquivo de legenda
    /// (`Filme.pt-BR.srt`) e não muda a língua.
    /// A contração é do país, não do namespace: "de Canadá" está errado em
    /// português e era o que a frase da curadoria dizia.
    #[test]
    fn o_pais_traz_o_proprio_artigo() {
        assert_eq!(de_pais("Canadá"), "do Canadá");
        assert_eq!(de_pais("França"), "da França");
        assert_eq!(de_pais("Estados Unidos"), "dos Estados Unidos");
        assert_eq!(de_pais("Filipinas"), "das Filipinas");
    }

    /// País sem artigo — e país que nem está na tabela de nomes, que chega
    /// aqui com o nome cru do provider — levam "de".
    #[test]
    fn pais_sem_artigo_leva_de_seco() {
        assert_eq!(de_pais("Portugal"), "de Portugal");
        assert_eq!(de_pais("Israel"), "de Israel");
        assert_eq!(de_pais("Ruritânia"), "de Ruritânia");
    }

    /// Todo nome de `PAISES` tem de ser reconhecido por `de_pais` — ou como
    /// contração, ou como "de" **de propósito**. O jeito de garantir isso é
    /// escrever o "de propósito": esta lista é a dos que dispensam artigo.
    #[test]
    fn nenhum_pais_conhecido_cai_no_de_por_esquecimento() {
        const SEM_ARTIGO: &[&str] = &[
            "Portugal",
            "Israel",
            "Hong Kong",
            "Taiwan",
            "Singapura",
            "Luxemburgo",
        ];
        for (_, nome) in PAISES {
            let tem_artigo = ARTIGO_DO_PAIS.iter().any(|(pais, _)| pais == nome);
            let e_de_proposito = SEM_ARTIGO.contains(nome);
            assert!(
                tem_artigo || e_de_proposito,
                "{nome} não tem contração nem está na lista dos que dispensam artigo"
            );
        }
    }

    #[test]
    fn a_regiao_nao_muda_a_lingua() {
        assert_eq!(idioma("pt-BR").as_deref(), Some("português"));
        assert_eq!(idioma("en_US").as_deref(), Some("inglês"));
    }

    /// Os países históricos importam num acervo de cinema: metade dos clássicos
    /// do leste europeu vem com `SU` ou `CS`.
    #[test]
    fn paises_que_nao_existem_mais_estao_na_tabela() {
        assert_eq!(pais("SU", "Soviet Union"), "União Soviética");
        assert_eq!(pais("YU", "Yugoslavia"), "Iugoslávia");
    }
}
