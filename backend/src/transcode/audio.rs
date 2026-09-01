//! Faixas de áudio: listar as do arquivo e escolher qual entra na sessão.
//!
//! **O dual audio sumia antes de chegar ao aparelho.** O `build_args` mapeava
//! `0:a:0?` fixo, então o ffmpeg punha uma faixa só na playlist e o player não
//! tinha o que oferecer — a lista que ele lê é a da playlist, não a do arquivo.
//! O botão de áudio ficava escondido justamente nos filmes que têm duas faixas.
//!
//! E não é coincidência que seja *justamente* nesses. Medido neste acervo,
//! **3.469 arquivos têm duas ou mais faixas**, e o formato recorrente é
//! `ac3:por | aac:eng`: a dublagem em PT-BR costuma ser ac3/eac3, que é
//! exatamente o codec que força a transcodificação em primeiro lugar. Quem toca
//! direto (celular que declara ac3) nunca viu o problema, porque vira direct
//! play e o arquivo vai inteiro, com as duas faixas dentro.
//!
//! Por isso o índice atravessa até o `decide`: escolher a faixa 1 de um
//! `ac3:por | aac:eng` muda o veredito de transcode pra direct play. Decidir
//! pelo codec da faixa 0 e tocar a 1 faria o selo mentir sobre o que vai tocar
//! — que é metade do pedido.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct AudioTrack {
    /// Índice ENTRE AS FAIXAS DE ÁUDIO (`0:a:N`), que é o que o `-map` do
    /// ffmpeg espera. Não confundir com o índice absoluto do stream — num
    /// arquivo com vídeo na posição 0, a primeira faixa de áudio é o stream 1 e
    /// mesmo assim `0:a:0`. É a mesma convenção do `SubtitleTrack::index`.
    pub index: i32,
    pub codec: String,
    pub language: Option<String>,
    pub title: Option<String>,
    /// 2 = estéreo, 6 = 5.1. O seletor mostra isso porque é o que distingue
    /// duas faixas do mesmo idioma.
    pub channels: Option<i64>,
    pub default: bool,
    pub forced: bool,
    /// Rótulo pronto pro seletor. Vem do servidor pelo mesmo motivo que o da
    /// legenda: pra os quatro clientes não reimplementarem a regra cada um do
    /// seu jeito.
    pub label: String,
}

/// Quantos canais, dito como as pessoas dizem.
fn canais(channels: Option<i64>) -> Option<&'static str> {
    match channels? {
        1 => Some("mono"),
        2 => Some("estéreo"),
        6 => Some("5.1"),
        8 => Some("7.1"),
        _ => None,
    }
}

/// Mesma escada do `label_for` das legendas — título, senão idioma, senão a
/// posição — com os canais no fim, que é o que separa duas faixas `por`.
///
/// **O idioma entra pelo nome, não pelo código.** O seletor mostrava
/// `por (5.1)` porque o `language` da tag do container é ISO 639-2 e ia cru pro
/// rótulo. Traduzir aqui e não na tela é a mesma decisão que fez o `label`
/// existir: um rótulo composto é do servidor, senão os quatro clientes
/// reimplementam a tabela de idiomas cada um do seu jeito — e o `language` cru
/// continua no JSON, ao lado, pra quem quiser casar por código.
///
/// Código fora da tabela **cai nele mesmo** (§18): `byn (estéreo)` é feio e é
/// um pedido de acréscimo em `regiao::IDIOMAS`; "Faixa 2" seria esconder que o
/// arquivo diz alguma coisa.
fn label_for(
    title: &Option<String>,
    language: &Option<String>,
    index: i32,
    channels: Option<i64>,
) -> String {
    let base = title
        .clone()
        .or_else(|| {
            language
                .as_deref()
                .map(|iso| crate::metadata::regiao::idioma_capitalizado(iso).unwrap_or_else(|| iso.to_string()))
        })
        .unwrap_or_else(|| format!("Faixa {}", index + 1));
    match canais(channels) {
        Some(c) => format!("{base} ({c})"),
        None => base,
    }
}

pub fn from_probe(probe: &serde_json::Value) -> Vec<AudioTrack> {
    let Some(streams) = probe.get("streams").and_then(|s| s.as_array()) else {
        return Vec::new();
    };

    streams
        .iter()
        .filter(|stream| stream.get("codec_type").and_then(|t| t.as_str()) == Some("audio"))
        .enumerate()
        .map(|(index, stream)| {
            let codec = stream
                .get("codec_name")
                .and_then(|c| c.as_str())
                .unwrap_or("desconhecido")
                .to_string();

            let tags = stream.get("tags");
            let disposition = stream.get("disposition");
            let flag = |name: &str| {
                disposition
                    .and_then(|d| d.get(name))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0)
                    == 1
            };

            let language = tags
                .and_then(|t| t.get("language"))
                .and_then(|v| v.as_str())
                .filter(|v| *v != "und")
                .map(str::to_string);
            let title = tags
                .and_then(|t| t.get("title"))
                .and_then(|v| v.as_str())
                .map(str::to_string);
            let channels = stream.get("channels").and_then(|v| v.as_i64());

            AudioTrack {
                index: index as i32,
                label: label_for(&title, &language, index as i32, channels),
                language,
                title,
                channels,
                default: flag("default"),
                forced: flag("forced"),
                codec,
            }
        })
        .collect()
}

/// Qual faixa vai tocar de verdade.
///
/// **Sem pedido, a faixa 0** — e não a marcada como `default`. Não é descuido:
/// `0:a:0` é o que o `build_args` sempre mapeou e é de onde vem o
/// `media_file.audio_codec` que o `decide` lê há tempos (o `probe.rs` pega o
/// primeiro stream de áudio). Honrar a disposição `default` aqui mudaria, em
/// silêncio, o que toca em milhares de arquivos que ninguém pediu pra mudar.
/// Quem quer outra faixa agora tem como dizer.
///
/// Índice fora da faixa é erro, e não o vizinho mais próximo: `-map 0:a:9?`
/// tolera a ausência e produziria uma sessão **muda**, sem nada no log. Errado
/// e invisível é pior que recusado.
pub fn escolher(faixas: &[AudioTrack], pedido: Option<i32>) -> Result<Option<&AudioTrack>, String> {
    if faixas.is_empty() {
        // Arquivo mudo. O `?` do `-map` já tolera isso; pedir faixa nele é que
        // não faz sentido.
        return match pedido {
            None | Some(0) => Ok(None),
            Some(n) => Err(format!("o arquivo não tem faixa de áudio, e você pediu a {n}")),
        };
    }

    let escolhido = pedido.unwrap_or(0);
    faixas
        .iter()
        .find(|f| f.index == escolhido)
        .map(Some)
        .ok_or_else(|| {
            format!(
                "faixa de áudio {escolhido} não existe: o arquivo tem {} ({})",
                faixas.len(),
                faixas
                    .iter()
                    .map(|f| f.index.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// O caso recorrente do acervo: dublagem ac3 na frente, original aac atrás.
    fn dual() -> serde_json::Value {
        json!({"streams": [
            {"codec_type": "video", "codec_name": "h264"},
            {"codec_type": "audio", "codec_name": "ac3", "channels": 6,
             "tags": {"language": "por"}, "disposition": {"default": 1}},
            {"codec_type": "audio", "codec_name": "aac", "channels": 2,
             "tags": {"language": "eng"}},
            {"codec_type": "subtitle", "codec_name": "subrip"}
        ]})
    }

    #[test]
    fn indice_e_relativo_ao_audio_nao_aos_streams() {
        let faixas = from_probe(&dual());
        assert_eq!(faixas.len(), 2);
        // o ac3 é o stream 1 do arquivo, mas é `0:a:0` pro ffmpeg
        assert_eq!(faixas[0].index, 0);
        assert_eq!(faixas[0].codec, "ac3");
        assert_eq!(faixas[1].index, 1);
        assert_eq!(faixas[1].codec, "aac");
    }

    #[test]
    fn video_e_legenda_nao_entram_na_lista() {
        assert!(from_probe(&dual()).iter().all(|f| f.codec != "h264" && f.codec != "subrip"));
    }

    #[test]
    fn rotulo_separa_duas_faixas_pelo_numero_de_canais() {
        let faixas = from_probe(&dual());
        assert_eq!(faixas[0].label, "Português (5.1)");
        assert_eq!(faixas[1].label, "Inglês (estéreo)");
    }

    /// O rótulo é dito; o código continua no JSON pra quem casa por código.
    #[test]
    fn o_rotulo_diz_o_idioma_e_o_campo_guarda_o_codigo() {
        let faixas = from_probe(&dual());
        assert_eq!(faixas[0].language.as_deref(), Some("por"));
        assert!(faixas[0].label.starts_with("Português"));
    }

    /// Código fora da tabela cai nele mesmo — `byn` existe neste acervo. Um
    /// "Faixa 2" esconderia que o arquivo diz alguma coisa (§18).
    #[test]
    fn idioma_fora_da_tabela_aparece_cru_em_vez_de_sumir() {
        let probe = json!({"streams": [
            {"codec_type": "audio", "codec_name": "aac", "channels": 2, "tags": {"language": "byn"}}
        ]});
        assert_eq!(from_probe(&probe)[0].label, "byn (estéreo)");
    }

    #[test]
    fn titulo_ganha_do_idioma_no_rotulo() {
        let probe = json!({"streams": [
            {"codec_type": "audio", "codec_name": "aac", "channels": 2,
             "tags": {"language": "por", "title": "Dublado"}}
        ]});
        assert_eq!(from_probe(&probe)[0].label, "Dublado (estéreo)");
    }

    #[test]
    fn idioma_indefinido_nao_vira_rotulo() {
        let probe = json!({"streams": [
            {"codec_type": "audio", "codec_name": "aac", "tags": {"language": "und"}}
        ]});
        let faixas = from_probe(&probe);
        assert_eq!(faixas[0].language, None);
        assert_eq!(faixas[0].label, "Faixa 1");
    }

    #[test]
    fn sem_pedido_toca_a_primeira() {
        let faixas = from_probe(&dual());
        let escolhida = escolher(&faixas, None).unwrap().unwrap();
        assert_eq!(escolhida.index, 0);
        assert_eq!(escolhida.codec, "ac3");
    }

    #[test]
    fn pedido_valido_escolhe_a_faixa_pedida() {
        let faixas = from_probe(&dual());
        let escolhida = escolher(&faixas, Some(1)).unwrap().unwrap();
        assert_eq!(escolhida.codec, "aac");
    }

    #[test]
    fn faixa_inexistente_e_recusada_e_nao_vira_sessao_muda() {
        let faixas = from_probe(&dual());
        let erro = escolher(&faixas, Some(9)).unwrap_err();
        assert!(erro.contains("9"), "{erro}");
    }

    #[test]
    fn arquivo_mudo_continua_tocando_sem_pedido() {
        let probe = json!({"streams": [{"codec_type": "video", "codec_name": "h264"}]});
        let faixas = from_probe(&probe);
        assert!(faixas.is_empty());
        assert!(escolher(&faixas, None).unwrap().is_none());
        assert!(escolher(&faixas, Some(1)).is_err());
    }

    #[test]
    fn probe_sem_streams_nao_explode() {
        assert!(from_probe(&json!({})).is_empty());
    }
}
