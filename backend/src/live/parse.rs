//! Parsers de M3U e XMLTV.
//!
//! Ambos são formatos de texto que qualquer provedor IPTV publica, e ambos são
//! mais bagunçados do que a especificação sugere. As duas funções aqui são
//! puras — recebem `&str`, devolvem structs — justamente pra poderem ser
//! testadas contra as formas reais sem subir nada.

use chrono::{DateTime, FixedOffset, TimeZone, Utc};

/// Um canal como o M3U descreve.
#[derive(Debug, Clone, PartialEq)]
pub struct CanalBruto {
    /// `tvg-id`: a chave que casa com o `channel=` do XMLTV.
    pub provider_key: String,
    pub name: String,
    pub number: Option<String>,
    pub logo_url: Option<String>,
    pub grupo: Option<String>,
    pub stream_url: String,
}

/// Um programa como o XMLTV descreve.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramaBruto {
    /// O `channel=` do XMLTV — casa com o `provider_key` do canal.
    pub canal: String,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub title: String,
    pub sub_title: Option<String>,
    pub description: Option<String>,
    /// `<date>` do XMLTV. Desempata título repetido na hora de achar a obra.
    pub year: Option<i32>,
    /// Primeira `<category>`. O ErsatzTV marca "Movie" nos filmes.
    pub categoria: Option<String>,
    /// A melhor imagem que o XMLTV oferece pra este programa, se oferecer.
    pub arte_url: Option<String>,
}

// ------------------------------------------------------------------- M3U

/// Lê um atributo `nome="valor"` de uma linha `#EXTINF`.
fn atributo(linha: &str, nome: &str) -> Option<String> {
    let chave = format!("{nome}=\"");
    let inicio = linha.find(&chave)? + chave.len();
    let resto = &linha[inicio..];
    let fim = resto.find('"')?;
    let valor = resto[..fim].trim();
    (!valor.is_empty()).then(|| valor.to_string())
}

/// Parseia um M3U estendido.
///
/// A forma é sempre a mesma: uma linha `#EXTINF:` com atributos e, **na linha
/// seguinte que não seja comentário**, a URL. Linhas em branco e outras
/// diretivas (`#EXTM3U`, `#EXTVLCOPT`) aparecem no meio e são ignoradas — é por
/// isso que a URL não pode ser lida como "a próxima linha" e sim como "a
/// próxima linha útil".
pub fn m3u(texto: &str) -> Vec<CanalBruto> {
    let mut canais = Vec::new();
    let mut pendente: Option<CanalBruto> = None;

    for linha in texto.lines() {
        let linha = linha.trim();
        if linha.is_empty() {
            continue;
        }

        if let Some(info) = linha.strip_prefix("#EXTINF:") {
            // O nome é o que vem depois da última vírgula. Atributos podem
            // conter vírgula dentro das aspas (`group-title="Filmes, Séries"`),
            // então cortar na PRIMEIRA vírgula erraria.
            let nome_exibicao = info.rsplit(',').next().unwrap_or("").trim().to_string();

            let provider_key = atributo(linha, "tvg-id")
                .or_else(|| atributo(linha, "channel-id"))
                .unwrap_or_default();

            let name = atributo(linha, "tvg-name")
                .filter(|n| !n.is_empty())
                .unwrap_or(nome_exibicao);

            pendente = Some(CanalBruto {
                provider_key,
                name,
                number: atributo(linha, "tvg-chno").or_else(|| atributo(linha, "channel-number")),
                logo_url: atributo(linha, "tvg-logo"),
                grupo: atributo(linha, "group-title"),
                stream_url: String::new(),
            });
            continue;
        }

        if linha.starts_with('#') {
            continue;
        }

        if let Some(mut canal) = pendente.take() {
            canal.stream_url = linha.to_string();
            // Sem `tvg-id` não há como casar com a grade nem como deduplicar
            // entre importações; a URL serve de chave por ser única na lista.
            if canal.provider_key.is_empty() {
                canal.provider_key = canal.stream_url.clone();
            }
            canais.push(canal);
        }
    }

    canais
}

/// Troca o host de uma URL de stream quando o provedor anuncia loopback.
///
/// O ErsatzTV publica `http://localhost:8409/iptv/channel/1.ts`. Dentro do
/// container do Odeon, `localhost` é o próprio container — o import passaria e
/// **todo canal falharia no play**. É a mesma armadilha do `10.0.2.2` no
/// emulador Android (§8e): a URL que o provedor escreve não é a URL de onde
/// você está.
///
/// A regra é comparativa, como o CORS do §10b: só reescreve o que é loopback, e
/// usa como destino o host de onde a lista veio — que é, por definição, um host
/// que este processo alcança, porque ele acabou de baixar a lista de lá.
pub fn reescreve_host(stream_url: &str, url_da_fonte: &str) -> String {
    let Some(host_alvo) = autoridade(url_da_fonte) else {
        return stream_url.to_string();
    };
    let Some(host_atual) = autoridade(stream_url) else {
        return stream_url.to_string();
    };

    let apenas_host = host_atual.split(':').next().unwrap_or("");
    let loopback = matches!(apenas_host, "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0");
    if !loopback {
        return stream_url.to_string();
    }

    // A porta do STREAM é preservada: um provedor pode servir a lista numa porta
    // e o vídeo em outra. Só o host muda.
    let porta = host_atual.split_once(':').map(|(_, p)| p.to_string());
    let host_novo = host_alvo.split(':').next().unwrap_or(&host_alvo).to_string();
    let autoridade_nova = match porta {
        Some(p) => format!("{host_novo}:{p}"),
        None => host_novo,
    };

    stream_url.replacen(&host_atual, &autoridade_nova, 1)
}

/// A parte `host:porta` de uma URL, sem depender de crate de URL.
fn autoridade(url: &str) -> Option<String> {
    let sem_esquema = url.split_once("://")?.1;
    let fim = sem_esquema.find(['/', '?', '#']).unwrap_or(sem_esquema.len());
    let autoridade = &sem_esquema[..fim];
    (!autoridade.is_empty()).then(|| autoridade.to_string())
}

// ----------------------------------------------------------------- XMLTV

/// `20260801210400 -0300` → instante em UTC.
///
/// O fuso é opcional na especificação; sem ele o horário é local do provedor, e
/// aqui assume-se UTC — chutar o fuso do servidor faria a grade escorregar sem
/// aviso, e UTC pelo menos erra de forma previsível.
fn instante(bruto: &str) -> Option<DateTime<Utc>> {
    let bruto = bruto.trim();
    let (data, fuso) = match bruto.split_once(' ') {
        Some((d, f)) => (d, Some(f)),
        None => (bruto, None),
    };
    if data.len() < 14 {
        return None;
    }

    let naive = chrono::NaiveDateTime::parse_from_str(&data[..14], "%Y%m%d%H%M%S").ok()?;

    match fuso {
        Some(f) if f.len() >= 5 => {
            let sinal = if f.starts_with('-') { -1 } else { 1 };
            let horas: i32 = f[1..3].parse().ok()?;
            let minutos: i32 = f[3..5].parse().ok()?;
            let offset = FixedOffset::east_opt(sinal * (horas * 3600 + minutos * 60))?;
            Some(offset.from_local_datetime(&naive).single()?.with_timezone(&Utc))
        }
        _ => Some(Utc.from_utc_datetime(&naive)),
    }
}

/// Desfaz as entidades que o XMLTV usa. A ordem importa: `&amp;` por último,
/// senão `&amp;lt;` viraria `<`.
fn desescapa(texto: &str) -> String {
    let mut saida = String::with_capacity(texto.len());
    let mut resto = texto;

    while let Some(i) = resto.find('&') {
        saida.push_str(&resto[..i]);
        let depois = &resto[i..];
        let Some(fim) = depois.find(';').filter(|f| *f <= 10) else {
            saida.push('&');
            resto = &depois[1..];
            continue;
        };
        let entidade = &depois[1..fim];
        let decodificada = match entidade {
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "amp" => Some('&'),
            n if n.starts_with("#x") || n.starts_with("#X") => {
                u32::from_str_radix(&n[2..], 16).ok().and_then(char::from_u32)
            }
            n if n.starts_with('#') => n[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match decodificada {
            Some(c) => saida.push(c),
            // Entidade desconhecida volta literal em vez de sumir.
            None => saida.push_str(&depois[..=fim]),
        }
        resto = &depois[fim + 1..];
    }

    saida.push_str(resto);
    saida
}

/// Conteúdo da primeira ocorrência de `<tag ...>...</tag>`.
fn elemento(corpo: &str, tag: &str) -> Option<String> {
    let abre = format!("<{tag}");
    let i = corpo.find(&abre)?;
    let depois = &corpo[i + abre.len()..];
    // Elemento vazio (`<desc/>`) não tem fechamento.
    if depois.starts_with("/>") {
        return None;
    }
    let inicio = depois.find('>')? + 1;
    let fecha = format!("</{tag}>");
    let fim = depois[inicio..].find(&fecha)?;
    let bruto = depois[inicio..inicio + fim].trim();
    (!bruto.is_empty()).then(|| desescapa(bruto))
}

/// O título como se escreve pra gente, não como o disco obrigou a escrever.
///
/// Nem todo provedor manda título de gente. O ErsatzTV manda o que a
/// biblioteca dele tem, e a biblioteca dele tem nome de arquivo — no canal de
/// clipes deste acervo, 30 dos 91 programas vêm com sósia Unicode no lugar de
/// `: ? | /`, e 15 vêm com o nome de release inteiro,
/// `Bo.Burnham.Inside.2021.1080p.WEBRip.x264…`.
///
/// Duas limpezas, e a segunda é conservadora de propósito: o parser de nome de
/// arquivo só entra quando o título **não tem espaço nenhum e tem ponto**, que
/// é a assinatura de um release. `Bee Gees - One Night Only - 1997 (Full
/// Concert HD)` é um título de verdade e passa intacto — mandá-lo pro parser
/// custaria o `(Full Concert HD)` sem ganhar nada.
fn titulo_de_gente(bruto: &str) -> String {
    let restaurado = crate::scanner::guess::restaura_o_que_o_disco_proibiu(bruto);
    let parece_release =
        !restaurado.contains(' ') && restaurado.matches('.').count() >= 2;
    if !parece_release {
        return restaurado;
    }
    let palpite = crate::scanner::guess::guess_from_filename(&restaurado);
    if palpite.title.trim().is_empty() {
        restaurado
    } else {
        palpite.title
    }
}

/// A melhor imagem que um `<programme>` oferece, pra usar de fundo.
///
/// O XMLTV do ErsatzTV manda três coisas, e elas não valem o mesmo:
///
/// - `<image type="still" orient="L">` — um quadro do episódio, **deitado**.
///   É o que um fundo quer: já tem a proporção da tela.
/// - `<image type="poster">` — em pé. Serve, cortado.
/// - `<icon src="…">` — em geral o mesmo pôster, e é o único que 392 dos 919
///   programas deste acervo têm.
///
/// Medido no XMLTV desta máquina: 827 dos 919 programas têm alguma imagem —
/// contra os 233 que hoje ganham arte por casamento de título com a
/// biblioteca. A foto sempre esteve ali; o Odeon é que a descartava.
fn melhor_imagem(corpo: &str) -> Option<String> {
    let mut deitada = None;
    let mut retrato = None;

    let mut resto = corpo;
    while let Some(i) = resto.find("<image") {
        let depois = &resto[i..];
        let Some(fim_abertura) = depois.find('>') else { break };
        let atributos = &depois[..fim_abertura];
        let Some(fim) = depois.find("</image>") else { break };
        let url = desescapa(depois[fim_abertura + 1..fim].trim());
        resto = &depois[fim + "</image>".len()..];

        // O ErsatzTV fecha a URL com uma aspa sobrando dentro do elemento.
        let url = url.trim_end_matches('"').to_string();
        if url.is_empty() {
            continue;
        }
        if atributo(atributos, "orient").as_deref() == Some("L") {
            deitada.get_or_insert(url);
        } else {
            retrato.get_or_insert(url);
        }
    }

    deitada.or(retrato).or_else(|| {
        let i = corpo.find("<icon")?;
        let depois = &corpo[i..];
        let fim = depois.find('>')?;
        atributo(&depois[..fim], "src").map(|u| desescapa(&u))
    })
}

/// Percorre um XMLTV recolhendo `<programme>`.
///
/// Varredura por delimitador em vez de um parser XML completo, e de propósito:
/// o formato é regular, o arquivo pode ter centenas de MB, e carregar uma
/// árvore inteira na memória pra ler quatro campos por nó seria caro. O que
/// custa correção — entidades — está isolado em `desescapa`.
pub fn xmltv(texto: &str) -> Vec<ProgramaBruto> {
    let mut programas = Vec::new();
    let mut resto = texto;

    while let Some(i) = resto.find("<programme") {
        let depois = &resto[i..];
        let Some(fim_abertura) = depois.find('>') else { break };
        let cabecalho = &depois[..fim_abertura];

        let Some(fim) = depois.find("</programme>") else { break };
        let corpo = &depois[fim_abertura + 1..fim];
        resto = &depois[fim + "</programme>".len()..];

        let (Some(inicio), Some(termino), Some(canal)) = (
            atributo(cabecalho, "start").and_then(|v| instante(&v)),
            atributo(cabecalho, "stop").and_then(|v| instante(&v)),
            atributo(cabecalho, "channel"),
        ) else {
            continue;
        };

        // Programa sem fim depois do início é dado corrompido, e entraria no
        // banco só pra violar o CHECK ou desenhar bloco de largura negativa.
        if termino <= inicio {
            continue;
        }

        programas.push(ProgramaBruto {
            canal: desescapa(&canal),
            starts_at: inicio,
            ends_at: termino,
            title: elemento(corpo, "title")
                .map(|t| titulo_de_gente(&t))
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| "—".into()),
            sub_title: elemento(corpo, "sub-title")
                .map(|t| crate::scanner::guess::restaura_o_que_o_disco_proibiu(&t)),
            description: elemento(corpo, "desc"),
            // `<date>` pode vir como "1999" ou "19990312"; só os 4 primeiros
            // dígitos interessam.
            year: elemento(corpo, "date")
                .and_then(|d| d.get(..4).and_then(|a| a.parse().ok()))
                .filter(|a: &i32| *a > 1870 && *a < 2200),
            categoria: elemento(corpo, "category"),
            arte_url: melhor_imagem(corpo),
        });
    }

    programas
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uma entrada real do ErsatzTV desta máquina.
    const EXTINF_REAL: &str = r#"#EXTINF:0 tvg-id="C1.145.ersatztv.org" channel-id="7FCpkKCqS0Ss0JrrjYiGqA" channel-number="1" CUID="7FCpkKCqS0Ss0JrrjYiGqA" tvg-chno="1" tvg-name="Tela Quente" tvg-logo="http://localhost:8409/iptv/logos/gen?text=Tela+Quente" group-title="ErsatzTV" tvc-stream-vcodec="h264" tvc-stream-acodec="aac", Tela Quente
http://localhost:8409/iptv/channel/1.ts"#;

    #[test]
    fn le_o_extinf_do_ersatztv() {
        let canais = m3u(EXTINF_REAL);
        assert_eq!(canais.len(), 1);
        let c = &canais[0];
        assert_eq!(c.provider_key, "C1.145.ersatztv.org");
        assert_eq!(c.name, "Tela Quente");
        assert_eq!(c.number.as_deref(), Some("1"));
        assert_eq!(c.grupo.as_deref(), Some("ErsatzTV"));
        assert_eq!(c.stream_url, "http://localhost:8409/iptv/channel/1.ts");
    }

    #[test]
    fn ignora_cabecalho_e_diretivas_no_meio() {
        let lista = "#EXTM3U url-tvg=\"x\"\n\
                     #EXTINF:-1 tvg-id=\"a\",Canal A\n\
                     #EXTVLCOPT:network-caching=1000\n\
                     \n\
                     http://exemplo/a.ts\n";
        let canais = m3u(lista);
        assert_eq!(canais.len(), 1);
        assert_eq!(canais[0].stream_url, "http://exemplo/a.ts");
    }

    #[test]
    fn nome_com_virgula_no_grupo_nao_quebra() {
        // Cortar na PRIMEIRA vírgula pegaria ` Séries" ...` como nome.
        let lista = "#EXTINF:-1 group-title=\"Filmes, Séries\" tvg-id=\"z\",Canal Z\n\
                     http://exemplo/z.ts";
        let canais = m3u(lista);
        assert_eq!(canais[0].grupo.as_deref(), Some("Filmes, Séries"));
        assert_eq!(canais[0].name, "Canal Z");
    }

    #[test]
    fn sem_tvg_id_a_url_vira_a_chave() {
        let canais = m3u("#EXTINF:-1,Sem Id\nhttp://exemplo/s.ts");
        assert_eq!(canais[0].provider_key, "http://exemplo/s.ts");
    }

    #[test]
    fn loopback_e_reescrito_pro_host_da_fonte() {
        let saida = reescreve_host(
            "http://localhost:8409/iptv/channel/1.ts",
            "http://172.17.0.1:8409/iptv/channels.m3u",
        );
        assert_eq!(saida, "http://172.17.0.1:8409/iptv/channel/1.ts");
    }

    #[test]
    fn a_porta_do_stream_e_preservada() {
        // Lista numa porta, vídeo em outra: só o host muda.
        let saida = reescreve_host(
            "http://127.0.0.1:9999/x.ts",
            "http://10.0.0.5:8409/lista.m3u",
        );
        assert_eq!(saida, "http://10.0.0.5:9999/x.ts");
    }

    #[test]
    fn host_de_verdade_nao_e_tocado() {
        let url = "http://cdn.provedor.com/live/42.m3u8";
        assert_eq!(reescreve_host(url, "http://172.17.0.1:8409/lista.m3u"), url);
    }

    #[test]
    fn le_programa_com_fuso_e_entidades() {
        let xml = r#"<tv><programme start="20260801210400 -0300" stop="20260801223300 -0300" channel="C10.193.ersatztv.org">
            <title lang="pt">Tr&#234;s Espi&#227;s Demais! &amp; cia</title>
            <sub-title>Uma Queda Por M&#250;sicos</sub-title>
            <desc>algo</desc></programme></tv>"#;
        let p = xmltv(xml);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].title, "Três Espiãs Demais! & cia");
        assert_eq!(p[0].sub_title.as_deref(), Some("Uma Queda Por Músicos"));
        assert_eq!(p[0].canal, "C10.193.ersatztv.org");
        // 21:04 em -03:00 é 00:04 UTC do dia seguinte.
        assert_eq!(p[0].starts_at.to_rfc3339(), "2026-08-02T00:04:00+00:00");
    }

    #[test]
    fn le_ano_e_categoria() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>Carandiru</title><date>2003</date>
            <category lang="en">Movie</category><category>Drama</category></programme></tv>"#;
        let p = &xmltv(xml)[0];
        assert_eq!(p.year, Some(2003));
        assert_eq!(p.categoria.as_deref(), Some("Movie"));
    }

    /// O que o ErsatzTV manda de verdade: `<icon>`, `<image type="poster">` e
    /// `<image type="still" orient="L">`, nessa ordem. Ganha a deitada.
    #[test]
    fn prefere_a_imagem_deitada() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>Uma Família da Pesada</title>
            <icon src="http://x/icone.jpg"/>
            <image type="poster" size="3">http://x/poster.jpg"</image>
            <image type="still" orient="L" size="3">http://x/quadro.jpg"</image>
            </programme></tv>"#;
        // A aspa sobrando no fim é do ErsatzTV, não erro de digitação: ele
        // fecha a URL com `"` dentro do elemento.
        assert_eq!(xmltv(xml)[0].arte_url.as_deref(), Some("http://x/quadro.jpg"));
    }

    #[test]
    fn cai_pro_icone_quando_nao_ha_image() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>t</title><icon src="http://x/i.jpg?a=1&amp;b=2"/></programme></tv>"#;
        assert_eq!(xmltv(xml)[0].arte_url.as_deref(), Some("http://x/i.jpg?a=1&b=2"));
    }

    /// Os títulos reais do canal de clipes deste acervo.
    #[test]
    fn titulo_com_sosia_de_caractere_proibido_e_restaurado() {
        assert_eq!(titulo_de_gente("Happy Tree Friends： Still Alive"),
                   "Happy Tree Friends: Still Alive");
        assert_eq!(titulo_de_gente("Snow What？ That's What"), "Snow What? That's What");
        assert_eq!(titulo_de_gente("AC⧸DC - Back In Black (Official 4K Video)"),
                   "AC/DC - Back In Black (Official 4K Video)");
    }

    #[test]
    fn nome_de_release_vira_titulo() {
        assert_eq!(
            titulo_de_gente("Bo.Burnham.Inside.2021.1080p.WEBRip.x264.AAC5.1-[YTS.MX]"),
            "Bo Burnham Inside"
        );
    }

    /// O conservadorismo do `titulo_de_gente`: título de gente passa intacto,
    /// mesmo tendo ano e parênteses que o parser de arquivo cortaria.
    #[test]
    fn titulo_de_verdade_nao_passa_pelo_parser_de_arquivo() {
        let bruto = "Bee Gees - One Night Only - 1997 (Full Concert HD)";
        assert_eq!(titulo_de_gente(bruto), bruto);
        let outro = "Mr. Robot";
        assert_eq!(titulo_de_gente(outro), outro);
    }

    #[test]
    fn sem_imagem_nenhuma_fica_none() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>Bee Gees</title></programme></tv>"#;
        assert_eq!(xmltv(xml)[0].arte_url, None);
    }

    /// Pedir 720 não é capricho: 220 de altura num fundo de 46vh borra.
    #[test]
    fn pede_a_imagem_maior_so_quando_da() {
        use crate::live::maior;
        assert_eq!(maior("http://x/i?tag=9&fillHeight=220"), "http://x/i?tag=9&fillHeight=720");
        assert_eq!(maior("http://x/i?fillHeight=440&z=1"), "http://x/i?fillHeight=720&z=1");
        // Já pede grande: não mexe.
        assert_eq!(maior("http://x/i?fillHeight=1080"), "http://x/i?fillHeight=1080");
        // Provedor que não conhecemos passa intacto.
        assert_eq!(maior("http://x/capa.jpg"), "http://x/capa.jpg");
    }

    #[test]
    fn data_completa_vira_so_o_ano() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>t</title><date>19990312</date></programme></tv>"#;
        assert_eq!(xmltv(xml)[0].year, Some(1999));
    }

    #[test]
    fn ano_absurdo_e_ignorado() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>t</title><date>0000</date></programme></tv>"#;
        assert_eq!(xmltv(xml)[0].year, None);
    }

    #[test]
    fn programa_sem_fim_valido_e_descartado() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801200000 +0000" channel="a">
            <title>ao contrário</title></programme></tv>"#;
        assert!(xmltv(xml).is_empty());
    }

    #[test]
    fn elemento_vazio_nao_vira_string_vazia() {
        let xml = r#"<tv><programme start="20260801210000 +0000" stop="20260801220000 +0000" channel="a">
            <title>t</title><desc/></programme></tv>"#;
        assert_eq!(xmltv(xml)[0].description, None);
    }

    #[test]
    fn entidade_desconhecida_volta_literal() {
        // Melhor um "&nbsp;" visível do que texto sumindo em silêncio.
        assert_eq!(desescapa("a&nbsp;b"), "a&nbsp;b");
    }

    #[test]
    fn sem_fuso_assume_utc() {
        let xml = r#"<tv><programme start="20260801210000" stop="20260801220000" channel="a">
            <title>t</title></programme></tv>"#;
        assert_eq!(xmltv(xml)[0].starts_at.to_rfc3339(), "2026-08-01T21:00:00+00:00");
    }
}
