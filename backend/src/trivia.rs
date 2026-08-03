//! Trivia sobre o filme — Wikidata e Wikipédia.
//!
//! O §32 fez curiosidades derivadas do acervo ("de Martin Campbell você também
//! tem…"). São únicas deste servidor, mas não são o que alguém quer dizer com
//! *curiosidade sobre o filme*: aquilo fala da sua estante, não da obra.
//!
//! Este módulo é a outra metade, e ele existe porque uma das três fontes que o
//! §32 descartou foi descartada rápido demais. Revisitadas:
//!
//! | fonte | veredito |
//! |---|---|
//! | TMDB / AniList | continua sem trivia — devolvem ficha |
//! | **Wikidata** | **é a resposta.** Estruturado, CC0, e casa pelo id do TMDB |
//! | **Wikipédia** | prosa de verdade, CC BY-SA — exige crédito e link |
//! | um LLM gerando | continua fora, e o §18 continua sendo a razão |
//!
//! ### Por que o Wikidata resolve o problema que a Wikipédia sozinha não resolve
//!
//! A propriedade `P4947` é o **id do filme no TMDB**. Isso significa que o
//! casamento é exato: o Odeon já tem esse id em `work.external_ids` desde o M1,
//! e não há busca por título nem desempate por ano. É o mesmo princípio do
//! `provider_key` do §8h — e é o que separa isto do casamento conservador da
//! grade ao vivo (§17), que precisa recusar 734 títulos ambíguos justamente por
//! não ter um id.
//!
//! Medido em 12 filmes sorteados do acervo: **12 de 12 casaram.**
//!
//! E o mesmo Wikidata devolve o **link do artigo da Wikipédia** por sitelink,
//! então nem o título do artigo é adivinhado.
//!
//! ### O que é dito, e o que é calado
//!
//! Cada fato só vira frase se for verificável e completo. O caso que mais
//! obrigou cuidado foi o dinheiro: `wdt:P2130` devolve o número **sem a
//! moeda**, e escrever "US$" num orçamento em euros é exatamente a mentira com
//! cara de metadado que o §18 proíbe. Então a moeda é buscada pelo caminho
//! completo do statement (`psv:`), e orçamento em moeda que não sabemos nomear
//! simplesmente não vira curiosidade.

use serde::{Deserialize, Serialize};

/// Uma curiosidade pronta pra tela.
///
/// `fonte` e `fonte_url` só existem no que vem da Wikipédia: a licença é
/// CC BY-SA e crédito não é opcional. O Wikidata é CC0 e não exige nada — mas
/// o link vai junto de qualquer forma, porque uma curiosidade que não se deixa
/// conferir é a mesma adivinhação que o §8b recusa no score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Curiosidade {
    pub tipo: String,
    pub texto: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fonte: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fonte_url: Option<String>,
}

impl Curiosidade {
    pub fn nova(tipo: &str, texto: String) -> Self {
        Self { tipo: tipo.to_string(), texto, fonte: None, fonte_url: None }
    }

    fn com_fonte(mut self, fonte: &str, url: Option<String>) -> Self {
        self.fonte = Some(fonte.to_string());
        self.fonte_url = url;
        self
    }
}

#[derive(Deserialize)]
struct SparqlResp {
    results: SparqlResults,
}

#[derive(Deserialize)]
struct SparqlResults {
    bindings: Vec<serde_json::Value>,
}

fn campo(linha: &serde_json::Value, nome: &str) -> Option<String> {
    linha
        .get(nome)?
        .get("value")?
        .as_str()
        .map(|s| s.to_string())
}

/// A política do Wikidata, dita pelo próprio serviço.
///
/// A primeira versão do aquecimento consultava **um filme por requisição**, com
/// 250 ms de pausa. O resultado, medido em 547 filmes: **513 falhas**, todas com
/// a mesma resposta —
///
/// ```text
/// 429 Please respect our robots policy and limit your requests to 1 RPS
/// ```
///
/// Duas lições, e a segunda é a que importa. A primeira: 250 ms é mais de 1 RPS.
/// A segunda: **a correção não é esperar mais, é perguntar menos vezes.** O
/// SPARQL aceita `VALUES` com uma lista de ids, então 547 filmes cabem em ~22
/// requisições em vez de 547 — o que é mais rápido *e* muito mais educado com um
/// serviço público e gratuito.
const LOTE: usize = 25;

/// Intervalo mínimo entre requisições ao WDQS. Um segundo é o que eles pedem.
const INTERVALO: std::time::Duration = std::time::Duration::from_millis(1100);

/// User-Agent descritivo, como a política da Wikimedia pede. `Odeon/0.1` sozinho
/// não diz o que é isto nem para que serve.
const UA: &str = "Odeon/0.1 (servidor de midia pessoal, uso nao comercial)";

/// O que o Wikidata sabe sobre um filme, antes de virar frase.
#[derive(Default)]
struct Dados {
    premios: Vec<String>,
    locais: Vec<String>,
    baseado: Option<String>,
    fotografia: Option<String>,
    orcamento: Option<(f64, String)>,
    bilheteria: Option<(f64, String)>,
    artigo: Option<String>,
    entidade: Option<String>,
}

/// Busca a trivia de um filme só. Atalho para o lote de um item — um caminho de
/// código, não dois.
pub async fn buscar(
    http: &reqwest::Client,
    tmdb_id: &str,
    titulo: &str,
) -> anyhow::Result<Vec<Curiosidade>> {
    let alvos = vec![(tmdb_id.to_string(), titulo.to_string())];
    Ok(buscar_lote(http, &alvos)
        .await?
        .remove(tmdb_id)
        .unwrap_or_default())
}

/// Busca a trivia de vários filmes numa requisição só.
///
/// Devolve um mapa id do TMDB → curiosidades. Filme sem entrada no Wikidata
/// simplesmente não aparece no mapa; quem chama trata isso como "procurei e não
/// há", que é resposta legítima.
pub async fn buscar_lote(
    http: &reqwest::Client,
    alvos: &[(String, String)],
) -> anyhow::Result<std::collections::HashMap<String, Vec<Curiosidade>>> {
    use std::collections::HashMap;

    // Os ids vão pra dentro de uma consulta SPARQL. Vêm do banco e deveriam ser
    // numéricos, mas "deveria" não é defesa — mesma postura do §8c, onde o único
    // SQL concatenado sai de uma whitelist.
    let validos: Vec<&(String, String)> = alvos
        .iter()
        .filter(|(id, _)| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
        .collect();
    if validos.is_empty() {
        return Ok(HashMap::new());
    }

    let lista = validos
        .iter()
        .map(|(id, _)| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(" ");

    let consulta = format!(
        r#"SELECT ?tmdb ?film ?prop ?valorLabel ?moedaLabel ?artigo WHERE {{
  VALUES ?tmdb {{ {lista} }}
  ?film wdt:P4947 ?tmdb .
  {{ ?film wdt:P166 ?valor . BIND("premio" AS ?prop) }}
  UNION {{ ?film wdt:P915 ?valor . BIND("local" AS ?prop) }}
  UNION {{ ?film wdt:P144 ?valor . BIND("baseado" AS ?prop) }}
  UNION {{ ?film wdt:P344 ?valor . BIND("fotografia" AS ?prop) }}
  UNION {{
    ?film p:P2130/psv:P2130 ?n . ?n wikibase:quantityAmount ?valor .
    OPTIONAL {{ ?n wikibase:quantityUnit ?moeda }} BIND("orcamento" AS ?prop)
  }}
  UNION {{
    ?film p:P2142/psv:P2142 ?n . ?n wikibase:quantityAmount ?valor .
    OPTIONAL {{ ?n wikibase:quantityUnit ?moeda }} BIND("bilheteria" AS ?prop)
  }}
  OPTIONAL {{ ?artigo schema:about ?film ; schema:isPartOf <https://pt.wikipedia.org/> }}
  SERVICE wikibase:label {{ bd:serviceParam wikibase:language "pt,en". }}
}} LIMIT 5000"#
    );

    let resp = pedir_sparql(http, &consulta).await?;

    let mut por_filme: HashMap<String, Dados> = HashMap::new();
    for linha in &resp.results.bindings {
        let Some(tmdb) = campo(linha, "tmdb") else { continue };
        let d = por_filme.entry(tmdb).or_default();
        if d.artigo.is_none() {
            d.artigo = campo(linha, "artigo");
        }
        if d.entidade.is_none() {
            d.entidade = campo(linha, "film");
        }
        let (Some(prop), Some(valor)) = (campo(linha, "prop"), campo(linha, "valorLabel")) else {
            continue;
        };
        let moeda = campo(linha, "moedaLabel").unwrap_or_default();
        match prop.as_str() {
            "premio" if !d.premios.contains(&valor) => d.premios.push(valor),
            "local" if !d.locais.contains(&valor) => d.locais.push(valor),
            "baseado" => {
                d.baseado.get_or_insert(valor);
            }
            "fotografia" => {
                d.fotografia.get_or_insert(valor);
            }
            "orcamento" => {
                if let Ok(n) = valor.parse::<f64>() {
                    d.orcamento.get_or_insert((n, moeda));
                }
            }
            "bilheteria" => {
                if let Ok(n) = valor.parse::<f64>() {
                    d.bilheteria.get_or_insert((n, moeda));
                }
            }
            _ => {}
        }
    }

    // A prosa da Wikipédia, também em lote: a API aceita até 20 títulos por
    // chamada com `exlimit`. Mesma razão do `VALUES` acima.
    let artigos: Vec<(String, String)> = por_filme
        .iter()
        .filter_map(|(tmdb, d)| d.artigo.clone().map(|a| (tmdb.clone(), a)))
        .collect();
    let producoes = producao_em_lote(http, &artigos).await.unwrap_or_default();

    let mut saida: HashMap<String, Vec<Curiosidade>> = HashMap::new();
    for (tmdb, titulo) in &validos {
        let Some(d) = por_filme.get(tmdb) else { continue };
        let mut itens = montar(d, titulo);
        if let Some(texto) = producoes.get(tmdb) {
            itens.push(
                Curiosidade::nova("producao", texto.clone())
                    .com_fonte("Wikipédia", d.artigo.clone()),
            );
        }
        saida.insert(tmdb.clone(), itens);
    }
    Ok(saida)
}

/// Uma requisição ao WDQS, com o intervalo e o recuo que a política pede.
///
/// O 429 não é tratado como erro definitivo: ele é a resposta esperada quando
/// se pede rápido demais, e insistir na mesma cadência transformaria uma
/// requisição recusada em quinhentas.
async fn pedir_sparql(http: &reqwest::Client, consulta: &str) -> anyhow::Result<SparqlResp> {
    let mut espera = std::time::Duration::from_secs(5);
    for tentativa in 0..4 {
        let r = http
            .get("https://query.wikidata.org/sparql")
            .query(&[("query", consulta), ("format", "json")])
            .header("Accept", "application/sparql-results+json")
            .header(reqwest::header::USER_AGENT, UA)
            .timeout(std::time::Duration::from_secs(60))
            .send()
            .await?;

        if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            if tentativa == 3 {
                anyhow::bail!("Wikidata recusou por excesso de requisições");
            }
            tracing::warn!(?espera, "429 do Wikidata — recuando");
            tokio::time::sleep(espera).await;
            espera *= 3;
            continue;
        }
        return Ok(r.error_for_status()?.json::<SparqlResp>().await?);
    }
    anyhow::bail!("Wikidata inalcançável")
}

/// Monta as frases a partir do que o Wikidata devolveu.
fn montar(d: &Dados, titulo: &str) -> Vec<Curiosidade> {
    let mut achadas = Vec::new();

    // --- prêmios ---------------------------------------------------------
    //
    // 23 linhas de prêmio não é curiosidade, é currículo. O que interessa é o
    // que a pessoa reconhece — então há uma lista de prestígio, e o resto vira
    // contagem. Sem ela, "ganhou o Dallas-Fort Worth Film Critics Association
    // Award" seria a manchete de Pulp Fiction.
    if !d.premios.is_empty() {
        const PRESTIGIO: [&str; 7] = [
            "scar", "Palma de Ouro", "BAFTA", "Golden Globe", "Globo de Ouro",
            "Urso de Ouro", "Leão de Ouro",
        ];
        let limpos: Vec<String> = d.premios.iter().map(|p| nome_do_premio(p)).collect();
        let destaque = limpos
            .iter()
            .find(|p| PRESTIGIO.iter().any(|alvo| p.contains(alvo)))
            .cloned();
        let total = limpos.len();
        achadas.push(Curiosidade::nova(
            "premio",
            match (destaque, total) {
                (Some(p), 1) => format!("Ganhou o {p}."),
                (Some(p), n) => format!("Ganhou o {p} — e mais {} prêmios.", n - 1),
                (None, 1) => format!("Ganhou o {}.", limpos[0]),
                (None, n) => format!("Levou {n} prêmios, entre eles o {}.", limpos[0]),
            },
        ));
    }

    // --- dinheiro ---------------------------------------------------------
    //
    // A relação entre os dois é o que diverte; o número solto é ficha. E a
    // moeda é obrigatória: escrever "US$" num orçamento em euros seria mentir
    // com cara de metadado.
    if let (Some((custo, m1)), Some((renda, _))) = (&d.orcamento, &d.bilheteria) {
        if let Some(simbolo) = simbolo_de(m1) {
            if *custo > 0.0 && *renda > 0.0 {
                let vezes = renda / custo;
                let comparacao = if vezes >= 2.0 {
                    format!(" — {} vezes o que custou", vezes.round() as i64)
                } else if vezes < 1.0 {
                    " — menos do que custou".to_string()
                } else {
                    String::new()
                };
                achadas.push(Curiosidade::nova(
                    "dinheiro",
                    format!(
                        "Custou {} e arrecadou {}{comparacao}.",
                        dinheiro(*custo, simbolo),
                        dinheiro(*renda, simbolo)
                    ),
                ));
            }
        }
    }

    if let Some(obra) = &d.baseado {
        let nome = sem_desambiguador(obra);
        achadas.push(Curiosidade::nova(
            "baseado",
            // "É uma adaptação de Drive", na ficha de Drive, lê como defeito.
            // O livro tem o mesmo nome — e é isso que a frase deve dizer.
            if nome.eq_ignore_ascii_case(titulo.trim()) {
                "É uma adaptação da obra homônima.".to_string()
            } else {
                format!("É uma adaptação de {nome}.")
            },
        ));
    }

    if !d.locais.is_empty() {
        let lista = d.locais.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        achadas.push(Curiosidade::nova("local", format!("Foi filmado em {lista}.")));
    }

    if let Some(dp) = &d.fotografia {
        achadas.push(Curiosidade::nova(
            "fotografia",
            format!("A fotografia é de {dp}."),
        ));
    }

    // Tudo acima é Wikidata, que é CC0 — o link vai junto por conferência, não
    // por licença. E ele aponta para a ENTIDADE, não para uma busca: a primeira
    // versão mandava `Special:Search?search=680`, que procura o texto "680" e
    // não acha o filme. Link de conferência que não confere é pior que link
    // nenhum, pela mesma razão que o §8b exige que o score seja auditável.
    let url = d
        .entidade
        .as_deref()
        .and_then(|uri| uri.rsplit('/').next())
        .map(|q| format!("https://www.wikidata.org/wiki/{q}"));
    for c in &mut achadas {
        c.fonte = Some("Wikidata".to_string());
        c.fonte_url = url.clone();
    }

    achadas
}

/// A seção de produção dos artigos.
///
/// **Uma requisição por artigo, e não por engano.** A primeira versão pedia 20
/// títulos de uma vez com `exlimit=20`, e a própria API respondia:
///
/// ```text
/// "exlimit" was too large for a whole article extracts request, lowered to 1.
/// ```
///
/// Ou seja: extrato de artigo INTEIRO é um por requisição — `exlimit` só passa
/// de 1 quando se pede apenas a introdução, e a introdução não tem a seção de
/// produção. O sintoma foi silencioso e caro: 23 páginas voltavam, **zero com
/// texto**, e o aquecimento gravou 17 parágrafos onde deveria haver centenas.
/// O aviso estava na resposta o tempo todo, num campo que eu não lia.
///
/// A lição é a mesma da §R6d: verificar que a chamada respondeu 200 não é
/// verificar que ela trouxe o que se pediu.
async fn producao_em_lote(
    http: &reqwest::Client,
    artigos: &[(String, String)],
) -> anyhow::Result<std::collections::HashMap<String, String>> {
    use std::collections::HashMap;
    // **Também 1 RPS.** A primeira versão usava 120 ms, e a Wikipédia passou a
    // responder 429 no meio do aquecimento — o que só apareceu porque o número
    // de parágrafos gravados (14 em ~500 artigos) não fazia sentido nenhum.
    const PAUSA: std::time::Duration = std::time::Duration::from_millis(1000);

    let mut saida = HashMap::new();
    let mut recusas = 0usize;

    for (tmdb, url) in artigos {
        let Some(fragmento) = url.rsplit('/').next() else { continue };
        let titulo = titulo_legivel(fragmento);

        let mut espera = std::time::Duration::from_secs(5);
        for tentativa in 0..3 {
            let r = http
                .get("https://pt.wikipedia.org/w/api.php")
                .query(&[
                    ("action", "query"),
                    ("prop", "extracts"),
                    ("explaintext", "1"),
                    ("format", "json"),
                    ("redirects", "1"),
                    ("titles", titulo.as_str()),
                ])
                .header(reqwest::header::USER_AGENT, UA)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await;

            match r {
                Ok(resp) if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    if tentativa == 2 {
                        recusas += 1;
                        break;
                    }
                    tokio::time::sleep(espera).await;
                    espera *= 3;
                    continue;
                }
                Ok(resp) => {
                    let corpo: serde_json::Value =
                        resp.json().await.unwrap_or(serde_json::Value::Null);
                    if let Some(paginas) = corpo["query"]["pages"].as_object() {
                        for pagina in paginas.values() {
                            if let Some(texto) = pagina["extract"].as_str() {
                                if let Some(par) = producao_do_texto(texto) {
                                    saida.insert(tmdb.clone(), par);
                                }
                            }
                        }
                    }
                    break;
                }
                Err(_) => {
                    recusas += 1;
                    break;
                }
            }
        }
        tokio::time::sleep(PAUSA).await;
    }

    // **Falha contada, não engolida.** Era o silêncio que escondia o 429: o
    // aquecimento terminava "sem falhas" com quase nenhum parágrafo gravado.
    if recusas > 0 {
        tracing::warn!(recusas, artigos = artigos.len(), "artigos da Wikipédia não vieram");
    }
    Ok(saida)
}

/// O título como a Wikipédia o escreve, a partir do sitelink.
fn titulo_legivel(fragmento: &str) -> String {
    let mut s = fragmento.replace('_', " ");
    for (de, pra) in [("%28", "("), ("%29", ")"), ("%C3%A7", "ç"), ("%2C", ",")] {
        s = s.replace(de, pra);
    }
    s
}

/// O primeiro parágrafo da seção de produção, a partir do texto do artigo.
///
/// Corta **em fim de frase**, nunca em contagem de caracteres: texto truncado
/// no meio de uma palavra lê como defeito de renderização — é a mesma correção
/// que o verso da caixa da locadora exigiu na §21 ao trocar `overflow: hidden`
/// por `line-clamp`.
fn producao_do_texto(texto: &str) -> Option<String> {
    // A seção se chama "Produção" na maioria dos artigos e "Realização" ou
    // "Filmagens" em alguns; procurar pelos três cobre o acervo sem regex.
    let inicio = texto
        .find("== Produção")
        .or_else(|| texto.find("== Realização"))
        .or_else(|| texto.find("== Filmagens"))?;
    let corpo = &texto[inicio..];
    let corpo = corpo.split_once('\n').map(|(_, r)| r).unwrap_or(corpo);
    // Só o primeiro parágrafo. O `extract` da Wikipédia separa parágrafos por
    // UM `\n`, não dois — cortando por `\n\n` vinham três parágrafos colados,
    // que na tela viram um bloco com quebras no meio.
    let paragrafo = corpo
        .split('\n')
        .map(str::trim)
        .find(|p| p.len() > 80 && !p.starts_with("=="))?;
    Some(cortar_em_frase(paragrafo, 320))
}

/// Corta no último ponto final antes do limite.
fn cortar_em_frase(texto: &str, limite: usize) -> String {
    if texto.chars().count() <= limite {
        return texto.to_string();
    }
    let recorte: String = texto.chars().take(limite).collect();
    match recorte.rfind(". ") {
        Some(p) => recorte[..=p].trim().to_string(),
        None => format!("{}…", recorte.trim()),
    }
}

/// Wikidata devolve o rótulo da moeda, não o símbolo. Moeda que não sabemos
/// nomear faz o fato inteiro ser descartado — ver o cabeçalho.
fn simbolo_de(moeda: &str) -> Option<&'static str> {
    let m = moeda.to_lowercase();
    if m.contains("dollar") || m.contains("dólar") {
        Some("US$")
    } else if m.contains("euro") {
        Some("€")
    } else if m.contains("real") {
        Some("R$")
    } else if m.contains("pound") || m.contains("libra") {
        Some("£")
    } else {
        None
    }
}

fn dinheiro(valor: f64, simbolo: &str) -> String {
    if valor >= 1_000_000_000.0 {
        let b = valor / 1_000_000_000.0;
        format!("{simbolo} {:.1} bilhão", b).replace('.', ",")
    } else if valor >= 1_000_000.0 {
        format!("{simbolo} {} milhões", (valor / 1_000_000.0).round() as i64)
    } else {
        format!("{simbolo} {} mil", (valor / 1_000.0).round() as i64)
    }
}

/// O rótulo do prêmio, sem a casca de categoria da Wikipédia.
///
/// O rótulo em português do Wikidata muitas vezes é o nome de uma **categoria**
/// e não do prêmio: "premiados com o BAFTA de melhor roteiro original",
/// "Vencedores do MTV Movie Award de melhor filme". Escrito na ficha, saía
/// "Ganhou o premiados com o BAFTA…".
///
/// E a capitalização fica **como veio**. Uma versão anterior punha a inicial em
/// minúscula pra encaixar no meio da frase, e transformou "National Board of
/// Review" em "national Board of Review" — não há como distinguir nome próprio
/// de substantivo comum sem saber o que a palavra é, então o certo é não mexer.
fn nome_do_premio(premio: &str) -> String {
    const CASCAS: [&str; 6] = [
        "premiados com o ", "premiados com a ", "premiadas com o ",
        "Vencedores do ", "vencedores do ", "Vencedores da ",
    ];
    let mut nome = premio.trim();
    for casca in CASCAS {
        if let Some(resto) = nome.strip_prefix(casca) {
            nome = resto;
            break;
        }
    }
    nome.to_string()
}

/// "Casino Royale (livro)" → "Casino Royale". O Wikidata desambigua no rótulo,
/// e o parêntese só existe pra separar dois itens dele — numa frase ele é ruído.
fn sem_desambiguador(nome: &str) -> String {
    match nome.split_once(" (") {
        Some((antes, _)) => antes.trim().to_string(),
        None => nome.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dinheiro_escala() {
        assert_eq!(dinheiro(8_000_000.0, "US$"), "US$ 8 milhões");
        assert_eq!(dinheiro(1_200_000_000.0, "US$"), "US$ 1,2 bilhão");
        assert_eq!(dinheiro(800_000.0, "R$"), "R$ 800 mil");
    }

    #[test]
    fn moeda_desconhecida_e_recusada() {
        assert_eq!(simbolo_de("United States dollar"), Some("US$"));
        assert_eq!(simbolo_de("euro"), Some("€"));
        // Sem símbolo conhecido o fato inteiro é descartado, em vez de virar
        // um número sem moeda ou um "US$" chutado.
        assert_eq!(simbolo_de("yen"), None);
    }

    #[test]
    fn corte_respeita_a_frase() {
        let t = "Primeira frase aqui. Segunda frase que estoura o limite dado.";
        let c = cortar_em_frase(t, 30);
        assert!(c.ends_with('.'), "cortou no meio: {c}");
        assert_eq!(c, "Primeira frase aqui.");
    }

    #[test]
    fn casca_de_categoria_sai_do_premio() {
        assert_eq!(
            nome_do_premio("premiados com o BAFTA de melhor roteiro original"),
            "BAFTA de melhor roteiro original"
        );
        assert_eq!(
            nome_do_premio("Vencedores do MTV Movie Award de melhor filme"),
            "MTV Movie Award de melhor filme"
        );
        // Nome próprio não é tocado — nem a capitalização.
        assert_eq!(
            nome_do_premio("National Board of Review: Top Ten Films"),
            "National Board of Review: Top Ten Films"
        );
    }

    #[test]
    fn desambiguador_sai() {
        assert_eq!(sem_desambiguador("Casino Royale (livro)"), "Casino Royale");
        assert_eq!(sem_desambiguador("Drive"), "Drive");
    }
}

// ------------------------------------------------------------ aquecimento

/// O estado do aquecimento, no formato que o `job` guarda em `progress`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct AquecimentoStatus {
    pub running: bool,
    pub total: usize,
    pub feitos: usize,
    /// Filmes que renderam ao menos uma curiosidade.
    pub com_trivia: usize,
    /// Casaram no Wikidata mas não havia nada notável a dizer.
    pub sem_nada: usize,
    pub falhas: usize,
    pub atual: Option<String>,
}

/// Busca a trivia de todos os filmes de uma vez.
///
/// Até aqui a trivia chegava quando alguém abria a ficha (§33), o que é bom pro
/// caso comum e ruim pro acervo: o primeiro a abrir cada filme paga 1,5 s.
///
/// **É um `job`, e isso não é cerimônia.** São 548 filmes com duas chamadas
/// externas cada — a ordem de dez minutos. O §12 registra o preço de operações
/// longas viverem só na memória do processo, e o §21 registra que os reparos
/// síncronos são dívida conhecida. Este nasce do jeito certo: sobrevive a
/// restart, aparece no histórico e dá pra cancelar.
///
/// **É retomável de propósito**, pela mesma razão do `repair-series` (§21): o
/// `WHERE` só pega o que ainda falta. Rodar de novo continua de onde parou, e
/// uma execução cancelada não perde o que já buscou.
pub async fn aquecer(
    pool: sqlx::PgPool,
    http: reqwest::Client,
    mut job: Option<crate::jobs::Job>,
) {
    let alvos: Vec<(uuid::Uuid, String, String)> = sqlx::query_as(
        "SELECT w.id, w.external_ids->>'tmdb', w.title
         FROM work w
         LEFT JOIN work_trivia t ON t.work_id = w.id
         WHERE w.kind = 'movie'
           AND w.match_state IN ('auto', 'confirmed')
           AND w.external_ids ? 'tmdb'
           AND t.work_id IS NULL
         ORDER BY w.title",
    )
    .fetch_all(&pool)
    .await
    .unwrap_or_default();

    let mut s = AquecimentoStatus {
        running: true,
        total: alvos.len(),
        ..Default::default()
    };

    for bloco in alvos.chunks(LOTE) {
        // Cancelamento cooperativo, conferido a cada LOTE — que é o ponto
        // seguro: no meio de uma requisição não há o que preservar.
        let pediram_parar = match &job {
            Some(j) => j.cancelled().await,
            None => false,
        };
        if pediram_parar {
            s.running = false;
            // `finish` consome o job, então ele sai do Option em vez de ser
            // emprestado — é por isso que `job` é `mut` aqui.
            if let Some(j) = job.take() {
                j.finish(&s, "cancelled", None).await;
            }
            tracing::info!(feitos = s.feitos, "aquecimento de trivia cancelado");
            return;
        }

        s.atual = bloco.first().map(|(_, _, t)| t.clone());
        let consulta: Vec<(String, String)> = bloco
            .iter()
            .map(|(_, tmdb, titulo)| (tmdb.clone(), titulo.clone()))
            .collect();

        match buscar_lote(&http, &consulta).await {
            Ok(mapa) => {
                for (id, tmdb, _) in bloco {
                    let itens = mapa.get(tmdb).cloned().unwrap_or_default();
                    if itens.is_empty() {
                        s.sem_nada += 1;
                    } else {
                        s.com_trivia += 1;
                    }
                    // Grava mesmo vazio: "procurei e não há" é resposta, e sem
                    // ela este filme seria reconsultado a cada abertura da ficha.
                    let _ = sqlx::query(
                        "INSERT INTO work_trivia (work_id, itens, buscado_em)
                         VALUES ($1, $2, now())
                         ON CONFLICT (work_id) DO UPDATE
                           SET itens = EXCLUDED.itens, buscado_em = now()",
                    )
                    .bind(id)
                    .bind(serde_json::to_value(&itens).unwrap_or_else(|_| serde_json::json!([])))
                    .execute(&pool)
                    .await;
                }
            }
            Err(e) => {
                // Falha NÃO é gravada — ver §33. O bloco continua elegível na
                // próxima passada, que é o que torna isto retomável de verdade.
                s.falhas += bloco.len();
                tracing::warn!(error = %e, "lote de trivia falhou");
            }
        }

        s.feitos += bloco.len();
        if let Some(j) = &job {
            j.tick(&s, s.feitos as i64, Some(s.total as i64)).await;
        }
        // A política do serviço, respeitada entre lotes.
        tokio::time::sleep(INTERVALO).await;
    }

    s.running = false;
    s.atual = None;
    tracing::info!(
        com_trivia = s.com_trivia,
        sem_nada = s.sem_nada,
        falhas = s.falhas,
        "aquecimento de trivia concluído"
    );
    if let Some(j) = job {
        j.finish(&s, "succeeded", None).await;
    }
}
