//! Negociação de codec — e o **porquê** dela.
//!
//! "Por que isso está transcodificando?" é a pergunta que todo mundo faz pro
//! Jellyfin e nunca recebe resposta direita. Aqui o plano de reprodução carrega
//! a lista de motivos, do mesmo jeito que o match do M1 e a recomendação do M5.
//!
//! A escada é sempre a mesma, do mais barato pro mais caro:
//!
//!   Direct Play    → o arquivo vai como está. Custo zero.
//!   Direct Stream  → remuxa o container, copia os streams. Custo quase zero.
//!   Transcode      → recodifica vídeo e/ou áudio. Custo alto.
//!
//! Via Tailscale nos aparelhos do próprio dono, Direct Play cobre quase tudo —
//! foi essa aposta que permitiu adiar este milestone até o fim.

use serde::{Deserialize, Serialize};

/// Acima disto o navegador engasga mesmo suportando o codec, e o ganho visual
/// numa TV doméstica é marginal. Serve de teto quando o cliente não declara um.
const DEFAULT_MAX_HEIGHT: i32 = 2160;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackMode {
    DirectPlay,
    DirectStream,
    Transcode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamAction {
    Copy,
    Encode,
}

/// O que o cliente diz saber tocar. Tudo opcional: cliente que não declara nada
/// recebe o padrão de navegador moderno.
#[derive(Debug, Clone, Deserialize)]
pub struct ClientCapabilities {
    #[serde(default = "default_containers")]
    pub containers: Vec<String>,
    #[serde(default = "default_video_codecs")]
    pub video_codecs: Vec<String>,
    #[serde(default = "default_audio_codecs")]
    pub audio_codecs: Vec<String>,
    #[serde(default)]
    pub max_height: Option<i32>,
    #[serde(default)]
    pub max_bitrate: Option<i64>,
    #[serde(default = "yes")]
    pub supports_hls: bool,
    /// Índice da faixa de legenda a queimar na imagem. Força transcode.
    #[serde(default)]
    pub burn_subtitle: Option<i32>,
    /// Faixa de áudio já resolvida contra o arquivo (`0:a:N`) — quem chama
    /// passa por `audio::escolher` antes, então aqui ela sempre existe.
    ///
    /// Fica junto do `burn_subtitle` porque é a mesma espécie de campo: não é
    /// uma capacidade do aparelho, é uma escolha de quem assiste que muda o
    /// plano. `None` só em arquivo sem áudio nenhum.
    #[serde(default)]
    pub audio_track: Option<i32>,
}

fn yes() -> bool {
    true
}

fn default_containers() -> Vec<String> {
    vec!["mp4".into(), "mov".into(), "webm".into()]
}

fn default_video_codecs() -> Vec<String> {
    vec!["h264".into(), "vp8".into(), "vp9".into()]
}

fn default_audio_codecs() -> Vec<String> {
    vec!["aac".into(), "mp3".into(), "opus".into(), "vorbis".into()]
}

impl Default for ClientCapabilities {
    fn default() -> Self {
        Self {
            containers: default_containers(),
            video_codecs: default_video_codecs(),
            audio_codecs: default_audio_codecs(),
            max_height: None,
            max_bitrate: None,
            supports_hls: true,
            burn_subtitle: None,
            audio_track: None,
        }
    }
}

/// O que o arquivo é, na prática. Vem direto do `media_file` preenchido no M0.
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub height: Option<i32>,
    pub bitrate: Option<i64>,
    /// Bits por amostra do vídeo — 8 no comum, 10 no HEVC Main 10 (R67).
    ///
    /// Sai do `pix_fmt` da `probe` e não de coluna própria: é a única
    /// informação de perfil que muda o veredito, e inventar uma coluna pra ela
    /// seria schema pra um caso. `None` quando a `probe` não diz, e aí a
    /// profundidade não barra nada — não saber não é motivo pra recodificar.
    pub video_bit_depth: Option<u8>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlaybackPlan {
    pub mode: PlaybackMode,
    pub video: StreamAction,
    pub audio: StreamAction,
    /// Altura de saída quando há downscale. `None` = mantém a original.
    pub target_height: Option<i32>,
    pub burn_subtitle: Option<i32>,
    /// Qual faixa de áudio vai tocar (`0:a:N`). É o campo que impede o selo de
    /// mentir: o `audio` acima diz *se* recodifica, este diz **o quê**.
    /// `None` em arquivo mudo.
    pub audio_track: Option<i32>,
    /// Por que este plano, em português. É a razão de este módulo existir.
    pub reasons: Vec<String>,
}

impl PlaybackPlan {
    pub fn needs_session(&self) -> bool {
        self.mode != PlaybackMode::DirectPlay
    }
}

/// ffprobe e navegador nem sempre chamam o codec pelo mesmo nome; isto
/// devolve o nome canônico dos dois lados.
pub(crate) fn codec_static(codec: &str) -> &'static str {
    let lower = codec.to_ascii_lowercase();
    // PCM não tem um nome: tem uma família. O ffprobe diz `pcm_s16le`,
    // `pcm_u8`, `pcm_s24le`…, e nenhum cliente declara essa lista — declara
    // "pcm". Sem esta linha, quem declarasse `pcm` não casaria com arquivo
    // nenhum da família.
    if lower.starts_with("pcm_") {
        return "pcm";
    }
    match lower.as_str() {
        // R67 — `hevc8` e `hevc10` são o **mesmo codec**, com a profundidade
        // dita junto. Quem decide o que fazer com ela é `profundidade_ok`;
        // aqui eles só precisam casar com `hevc`, senão um cliente que
        // declarasse `hevc10` não casaria com arquivo nenhum.
        "h265" | "hevc" | "h.265" | "hevc8" | "hevc10" | "hvc1" => "hevc",
        "h.264" | "avc" | "avc1" | "h264" => "h264",
        "mp4a" | "aac" => "aac",
        "eac3" | "e-ac-3" => "eac3",
        "ac3" => "ac3",
        "mp3" => "mp3",
        "opus" => "opus",
        "vorbis" => "vorbis",
        "vp8" => "vp8",
        "vp9" => "vp9",
        "av1" => "av1",
        "flac" => "flac",
        "alac" => "alac",
        "pcm" => "pcm",
        "dts" => "dts",
        "truehd" => "truehd",
        "matroska" => "matroska",
        "mp4" => "mp4",
        "mov" => "mov",
        "webm" => "webm",
        _ => "desconhecido",
    }
}

/// **`desconhecido` não casa com `desconhecido` (R49).** Foi assim que um
/// cliente honesto passou a receber áudio que não toca.
///
/// `codec_static` devolve `"desconhecido"` pro que não está no mapa — dos dois
/// lados. Então bastava o cliente declarar **um** codec que o mapa não conhecia
/// pra que **todo** codec desconhecido do acervo passasse a "bater": o `alac`
/// do iOS casava com `pcm_s16le`, `mp2` e `wmav2`, e o plano saía `audio=copy`
/// pra um aparelho que não decodifica nenhum dos três.
///
/// Medido no arquivo `Fifth Season Opening Theme` (hevc + `pcm_s16le`), com a
/// lista real do cliente iOS:
///
/// | lista declarada | antes | depois |
/// |---|---|---|
/// | `aac,ac3,eac3,opus,flac`      | `transcode`, `audio=encode` | igual |
/// | `aac,ac3,eac3,alac,opus,flac` | **`direct_stream`, `audio=copy`** | `transcode`, `audio=encode` |
///
/// O sintoma seria filme mudo — o defeito que o dono do cliente iOS descreveu
/// como *"um defeito que o usuário não consegue nem diagnosticar"*.
///
/// O casamento por **nome exato** continua: quem declarar `pcm_s16le` na letra
/// recebe `pcm_s16le`. O que sai é só o casamento por ignorância mútua.
fn supports(list: &[String], codec: &str) -> bool {
    let wanted = codec_static(codec);
    list.iter().any(|entry| {
        (wanted != "desconhecido" && codec_static(entry) == wanted)
            || entry.eq_ignore_ascii_case(codec)
    })
}

/// A profundidade de bits também é uma capacidade — **R67**.
///
/// ## O que aconteceu
///
/// Depois que a R66 pôs o HEVC em fMP4, o extrator do ExoPlayer passou a ler o
/// fluxo e o erro mudou de lugar:
///
/// ```text
/// MediaCodecVideoRenderer error · format_supported=NO_EXCEEDS_CAPABILITIES
/// hvc1.2.4.L120.90 · 10bit Luma/Chroma
/// ```
///
/// O aparelho tinha decodificador de `video/hevc` e **não** tinha o perfil
/// Main 10. "Toca HEVC" é uma resposta boa demais pra pergunta que estava
/// sendo feita: 5.319 dos arquivos de série deste acervo são Main 10.
///
/// ## O vocabulário, e por que `hevc` continua valendo tudo
///
/// | o cliente declara | um HEVC 8 bits | um HEVC 10 bits |
/// |---|---|---|
/// | `hevc` | copia | **copia** |
/// | `hevc10` | copia | copia |
/// | `hevc8` | copia | **recodifica** |
///
/// O pedido era `hevc` = 8 bits e `hevc10` = os dois. Não dá, e o motivo é
/// medido: **três clientes já mandam `hevc` hoje** — iOS, TV e web —, e o
/// AVPlayer e a TCL decodificam Main 10 sem problema. Estreitar o sentido da
/// palavra que já está no ar transformaria 5.319 arquivos de cópia em
/// recodificação da noite pro dia, em aparelhos que não precisam disso.
///
/// Então a precisão entra por palavra nova, nos dois sentidos: quem só faz 8
/// bits diz `hevc8` e ganha a proteção; quem faz os dois pode dizer `hevc10` e
/// documentar isso. `hevc` continua querendo dizer "HEVC, do jeito que sempre
/// quis" — e é o único jeito de a mudança não quebrar quem não pediu nada.
///
/// ⚠️ **O vocabulário é só do HEVC**, de propósito. O acervo tem 34 arquivos
/// h264 High 10, e eles têm o mesmo problema — mas `h2648` não é palavra que
/// alguém escreveria, e um esquema genérico (`sufixo 8`/`10`) quebraria `vp8`,
/// que já é um codec. Quando os 34 incomodarem, entram por nome.
fn profundidade_ok(list: &[String], codec: &str, bits: Option<u8>) -> bool {
    // Só o HEVC tem os dois perfis no vocabulário, e só acima de 8 bits há o
    // que perguntar.
    if codec_static(codec) != "hevc" || bits.unwrap_or(8) <= 8 {
        return true;
    }
    // Basta uma entrada de HEVC que **não** seja a de 8 bits. Se a lista só
    // tem `hevc8`, o cliente disse com todas as letras que não decodifica.
    list.iter()
        .filter(|entry| codec_static(entry) == "hevc")
        .any(|entry| !entry.eq_ignore_ascii_case("hevc8"))
}

/// `matroska` no ffprobe é `.mkv` pro resto do mundo, e nenhum navegador toca.
fn container_matches(list: &[String], container: &str) -> bool {
    let lowered = container.to_ascii_lowercase();
    let normalized = match lowered.as_str() {
        "matroska" | "matroska,webm" => "mkv",
        "mov,mp4,m4a,3gp,3g2,mj2" => "mp4",
        "quicktime" => "mov",
        other => other,
    };
    list.iter().any(|entry| {
        let entry = entry.to_ascii_lowercase();
        entry == normalized
            // mkv e webm compartilham container; se o cliente aceita webm e o
            // arquivo é matroska, ainda depende dos codecs — resolvido acima.
            || (normalized == "mkv" && entry == "mkv")
    })
}

pub fn plan(media: &MediaInfo, caps: &ClientCapabilities) -> PlaybackPlan {
    let mut reasons: Vec<String> = Vec::new();

    // --- vídeo -------------------------------------------------------------
    let video_codec = media.video_codec.as_deref().unwrap_or("desconhecido");
    let codec_ok = supports(&caps.video_codecs, video_codec);
    if !codec_ok {
        reasons.push(format!(
            "o cliente não toca vídeo em {}",
            codec_static(video_codec)
        ));
    }

    // R67 — o perfil também é capacidade. A frase diz a profundidade porque
    // "não toca hevc" seria mentira num aparelho que toca hevc 8 bits.
    let profundidade_ok = profundidade_ok(&caps.video_codecs, video_codec, media.video_bit_depth);
    if codec_ok && !profundidade_ok {
        reasons.push(format!(
            "o cliente toca {} em 8 bits, e este é de {} bits",
            codec_static(video_codec),
            media.video_bit_depth.unwrap_or(0)
        ));
    }
    let codec_ok = codec_ok && profundidade_ok;

    let max_height = caps.max_height.unwrap_or(DEFAULT_MAX_HEIGHT);
    let too_tall = media.height.map(|h| h > max_height).unwrap_or(false);
    let target_height = if too_tall { Some(max_height) } else { None };
    if too_tall {
        reasons.push(format!(
            "{}p acima do limite de {}p do cliente",
            media.height.unwrap_or(0),
            max_height
        ));
    }

    let too_fat = match (caps.max_bitrate, media.bitrate) {
        (Some(limit), Some(actual)) if actual > limit => {
            reasons.push(format!(
                "{} kbps acima do limite de {} kbps",
                actual / 1000,
                limit / 1000
            ));
            true
        }
        _ => false,
    };

    // Queimar legenda obriga a redesenhar o quadro — não há como copiar vídeo.
    let burn_subtitle = caps.burn_subtitle;
    if burn_subtitle.is_some() {
        reasons.push("legenda pedida queimada na imagem".to_string());
    }

    let video = if codec_ok && !too_tall && !too_fat && burn_subtitle.is_none() {
        StreamAction::Copy
    } else {
        StreamAction::Encode
    };

    // --- áudio -------------------------------------------------------------
    // O `media.audio_codec` daqui é o da faixa ESCOLHIDA, não o da primeira do
    // arquivo — quem chama resolve isso antes. É o que faz o veredito valer pro
    // que vai tocar: num `ac3:por | aac:eng`, pedir a faixa 1 tira o único
    // motivo de transcodificar e a resposta vira direct play de verdade.
    if let Some(faixa) = caps.audio_track.filter(|n| *n != 0) {
        reasons.push(format!("faixa de áudio {faixa} escolhida em vez da primeira"));
    }
    let audio_codec = media.audio_codec.as_deref().unwrap_or("desconhecido");
    let audio_ok = media.audio_codec.is_none() || supports(&caps.audio_codecs, audio_codec);
    if !audio_ok {
        reasons.push(format!(
            "o cliente não toca áudio em {}",
            codec_static(audio_codec)
        ));
    }
    let audio = if audio_ok {
        StreamAction::Copy
    } else {
        StreamAction::Encode
    };

    // --- container ---------------------------------------------------------
    let container = media.container.as_deref().unwrap_or("desconhecido");
    let container_ok = container_matches(&caps.containers, container);

    // --- o veredito --------------------------------------------------------
    let mode = if video == StreamAction::Copy && audio == StreamAction::Copy {
        if container_ok {
            reasons.push("codecs e container batem: vai o arquivo original".to_string());
            PlaybackMode::DirectPlay
        } else {
            reasons.push(format!(
                "codecs batem, mas o container {container} não serve: só remuxa"
            ));
            PlaybackMode::DirectStream
        }
    } else {
        PlaybackMode::Transcode
    };

    // Cliente sem HLS e que precisaria de sessão não tem saída: avisa em vez de
    // servir um arquivo que ele não vai tocar.
    if mode != PlaybackMode::DirectPlay && !caps.supports_hls {
        reasons.push("cliente não suporta HLS — não há caminho viável".to_string());
    }

    PlaybackPlan {
        mode,
        video,
        audio,
        target_height,
        burn_subtitle,
        audio_track: caps.audio_track,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn browser() -> ClientCapabilities {
        ClientCapabilities::default()
    }

    fn hevc_10bits() -> MediaInfo {
        MediaInfo {
            container: Some("matroska".into()),
            video_codec: Some("hevc".into()),
            audio_codec: Some("aac".into()),
            height: Some(1080),
            bitrate: Some(5_000_000),
            video_bit_depth: Some(10),
        }
    }

    fn caps_com(video: &[&str]) -> ClientCapabilities {
        ClientCapabilities {
            containers: vec!["mkv".into(), "mp4".into()],
            video_codecs: video.iter().map(|s| s.to_string()).collect(),
            audio_codecs: vec!["aac".into()],
            ..ClientCapabilities::default()
        }
    }

    /// **R67 — `hevc` continua valendo tudo.** Três clientes já mandam essa
    /// palavra hoje e decodificam Main 10; estreitá-la transformaria 5.319
    /// arquivos de cópia em recodificação sem ninguém pedir.
    #[test]
    fn hevc_sozinho_ainda_copia_10_bits() {
        let p = plan(&hevc_10bits(), &caps_com(&["hevc"]));
        assert_eq!(p.video, StreamAction::Copy, "{:?}", p.reasons);
    }

    /// Quem diz `hevc8` disse com todas as letras que não decodifica Main 10.
    #[test]
    fn hevc8_recodifica_o_de_10_bits() {
        let p = plan(&hevc_10bits(), &caps_com(&["hevc8"]));
        assert_eq!(p.video, StreamAction::Encode);
        assert!(
            p.reasons.iter().any(|r| r.contains("8 bits") && r.contains("10 bits")),
            "a frase tem de dizer a profundidade, senão vira 'não toca hevc' num aparelho que toca: {:?}",
            p.reasons
        );
    }

    /// E o mesmo cliente copia o de 8 bits — que é o ponto de ter a palavra.
    #[test]
    fn hevc8_copia_o_de_8_bits() {
        let mut media = hevc_10bits();
        media.video_bit_depth = Some(8);
        assert_eq!(plan(&media, &caps_com(&["hevc8"])).video, StreamAction::Copy);
    }

    /// `hevc10` é a declaração explícita de quem faz os dois.
    #[test]
    fn hevc10_copia_os_dois() {
        assert_eq!(plan(&hevc_10bits(), &caps_com(&["hevc10"])).video, StreamAction::Copy);
        let mut oito = hevc_10bits();
        oito.video_bit_depth = Some(8);
        assert_eq!(plan(&oito, &caps_com(&["hevc10"])).video, StreamAction::Copy);
    }

    /// Um aparelho que declara os dois — o caso do pedido — copia tudo.
    #[test]
    fn hevc8_mais_hevc10_copia_tudo() {
        assert_eq!(
            plan(&hevc_10bits(), &caps_com(&["hevc8", "hevc10"])).video,
            StreamAction::Copy
        );
    }

    /// **Não saber não recodifica.** `probe` sem `pix_fmt` deixa a
    /// profundidade nula, e nula não é motivo de nada.
    #[test]
    fn profundidade_desconhecida_nao_barra() {
        let mut media = hevc_10bits();
        media.video_bit_depth = None;
        assert_eq!(plan(&media, &caps_com(&["hevc8"])).video, StreamAction::Copy);
    }

    /// O portão é só do HEVC: um h264 de 10 bits não é barrado por `hevc8`,
    /// porque a palavra não fala dele.
    #[test]
    fn a_profundidade_so_vale_pro_hevc() {
        let mut media = hevc_10bits();
        media.video_codec = Some("h264".into());
        assert!(profundidade_ok(&["h264".to_string()], "h264", Some(10)));
    }

    fn mp4_h264() -> MediaInfo {
        MediaInfo {
            container: Some("mp4".into()),
            video_codec: Some("h264".into()),
            audio_codec: Some("aac".into()),
            height: Some(1080),
            bitrate: Some(5_000_000),
            video_bit_depth: Some(8),
        }
    }

    #[test]
    fn caso_comum_e_direct_play() {
        let plan = plan(&mp4_h264(), &browser());
        assert_eq!(plan.mode, PlaybackMode::DirectPlay);
        assert_eq!(plan.video, StreamAction::Copy);
        assert_eq!(plan.audio, StreamAction::Copy);
    }

    #[test]
    fn mkv_com_codecs_bons_so_remuxa() {
        let mut media = mp4_h264();
        media.container = Some("matroska".into());
        let plan = plan(&media, &browser());
        assert_eq!(plan.mode, PlaybackMode::DirectStream);
        // não recodifica nada — é o ponto do Direct Stream
        assert_eq!(plan.video, StreamAction::Copy);
        assert_eq!(plan.audio, StreamAction::Copy);
    }

    #[test]
    fn hevc_no_navegador_vira_transcode() {
        let mut media = mp4_h264();
        media.video_codec = Some("hevc".into());
        let plan = plan(&media, &browser());
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert_eq!(plan.video, StreamAction::Encode);
        assert!(plan.reasons.iter().any(|r| r.contains("hevc")));
    }

    #[test]
    fn audio_ruim_nao_recodifica_o_video() {
        let mut media = mp4_h264();
        media.audio_codec = Some("truehd".into());
        let plan = plan(&media, &browser());
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        // o vídeo continua sendo copiado — recodificar os dois é desperdício
        assert_eq!(plan.video, StreamAction::Copy);
        assert_eq!(plan.audio, StreamAction::Encode);
    }

    #[test]
    fn resolucao_acima_do_limite_faz_downscale() {
        let mut media = mp4_h264();
        media.height = Some(2160);
        let caps = ClientCapabilities {
            max_height: Some(1080),
            ..browser()
        };
        let plan = plan(&media, &caps);
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert_eq!(plan.target_height, Some(1080));
    }

    #[test]
    fn bitrate_acima_do_limite_transcodifica() {
        let caps = ClientCapabilities {
            max_bitrate: Some(2_000_000),
            ..browser()
        };
        let plan = plan(&mp4_h264(), &caps);
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert!(plan.reasons.iter().any(|r| r.contains("kbps")));
    }

    #[test]
    fn queimar_legenda_impede_copia_de_video() {
        let caps = ClientCapabilities {
            burn_subtitle: Some(0),
            ..browser()
        };
        let plan = plan(&mp4_h264(), &caps);
        assert_eq!(plan.mode, PlaybackMode::Transcode);
        assert_eq!(plan.video, StreamAction::Encode);
    }

    /// **A R49.** Declarar um codec que o mapa não conhece não pode fazer
    /// **todos** os desconhecidos passarem.
    #[test]
    fn desconhecido_nao_casa_com_desconhecido() {
        let mut media = mp4_h264();
        media.audio_codec = Some("pcm_s16le".into());

        // a lista real do cliente iOS, que declara `alac` — fora do mapa
        let ios = ClientCapabilities {
            audio_codecs: ["aac", "ac3", "eac3", "alac", "opus", "flac"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            ..browser()
        };
        let p = plan(&media, &ios);
        assert_eq!(p.audio, StreamAction::Encode, "alac casou com pcm_s16le");
        assert_eq!(p.mode, PlaybackMode::Transcode);

        // e o mesmo vale pros outros desconhecidos do acervo
        for codec in ["mp2", "wmav2", "pcm_u8"] {
            let mut m = mp4_h264();
            m.audio_codec = Some(codec.into());
            assert_eq!(
                plan(&m, &ios).audio,
                StreamAction::Encode,
                "{codec} passou por causa de um desconhecido declarado"
            );
        }
    }

    /// PCM é família, não codec: quem declara `pcm` tem que casar com
    /// `pcm_s16le` e companhia, senão a correção acima cortaria demais.
    #[test]
    fn pcm_e_uma_familia() {
        let caps = ClientCapabilities {
            audio_codecs: vec!["aac".into(), "pcm".into()],
            ..browser()
        };
        for codec in ["pcm_s16le", "pcm_u8", "pcm_s24le"] {
            let mut media = mp4_h264();
            media.audio_codec = Some(codec.into());
            assert_eq!(
                plan(&media, &caps).audio,
                StreamAction::Copy,
                "{codec} não casou com `pcm` declarado"
            );
        }
        // …e não arrasta outros desconhecidos junto
        let mut media = mp4_h264();
        media.audio_codec = Some("wmav2".into());
        assert_eq!(plan(&media, &caps).audio, StreamAction::Encode);
    }

    /// O nome exato continua valendo: é a saída de quem quer declarar um codec
    /// que o mapa não conhece **sem** abrir a porta pros outros.
    #[test]
    fn nome_exato_ainda_casa() {
        let caps = ClientCapabilities {
            audio_codecs: vec!["aac".into(), "mp2".into()],
            ..browser()
        };
        let mut media = mp4_h264();
        media.audio_codec = Some("mp2".into());
        assert_eq!(plan(&media, &caps).audio, StreamAction::Copy);
    }

    /// `alac` entrou no mapa, então declarar `alac` casa com arquivo `alac`.
    #[test]
    fn alac_agora_e_conhecido() {
        let caps = ClientCapabilities {
            audio_codecs: vec!["aac".into(), "alac".into()],
            ..browser()
        };
        let mut media = mp4_h264();
        media.audio_codec = Some("alac".into());
        assert_eq!(plan(&media, &caps).audio, StreamAction::Copy);
    }

    #[test]
    fn sempre_ha_um_motivo() {
        for media in [mp4_h264(), MediaInfo::default()] {
            assert!(!plan(&media, &browser()).reasons.is_empty());
        }
    }

    #[test]
    fn arquivo_sem_audio_nao_e_motivo_de_transcode() {
        let mut media = mp4_h264();
        media.audio_codec = None;
        assert_eq!(plan(&media, &browser()).audio, StreamAction::Copy);
    }

    /// O caso que motivou o índice de faixa, e o mais comum do acervo:
    /// `ac3:por | aac:eng`. A faixa escolhida decide o veredito, e o plano diz
    /// qual delas vai tocar — senão o selo mente.
    #[test]
    fn a_faixa_escolhida_e_quem_decide_o_veredito() {
        // faixa 0, a dublagem em ac3: o navegador não toca, então transcodifica
        let mut dublado = mp4_h264();
        dublado.audio_codec = Some("ac3".into());
        let caps_zero = ClientCapabilities { audio_track: Some(0), ..browser() };
        let p = plan(&dublado, &caps_zero);
        assert_eq!(p.mode, PlaybackMode::Transcode);
        assert_eq!(p.audio, StreamAction::Encode);
        assert_eq!(p.audio_track, Some(0));

        // faixa 1, o original em aac: some o único motivo de transcodificar
        let mut original = mp4_h264();
        original.audio_codec = Some("aac".into());
        let caps_um = ClientCapabilities { audio_track: Some(1), ..browser() };
        let p = plan(&original, &caps_um);
        assert_eq!(p.mode, PlaybackMode::DirectPlay);
        assert_eq!(p.audio, StreamAction::Copy);
        assert_eq!(p.audio_track, Some(1));
    }

    #[test]
    fn escolher_outra_faixa_aparece_nos_motivos() {
        let caps = ClientCapabilities { audio_track: Some(1), ..browser() };
        assert!(plan(&mp4_h264(), &caps)
            .reasons
            .iter()
            .any(|r| r.contains("faixa de áudio 1")));
    }

    /// A primeira faixa é o que sempre tocou; dizer isso nos motivos seria
    /// ruído em toda reprodução.
    #[test]
    fn a_faixa_padrao_nao_vira_motivo() {
        let caps = ClientCapabilities { audio_track: Some(0), ..browser() };
        assert!(!plan(&mp4_h264(), &caps)
            .reasons
            .iter()
            .any(|r| r.contains("faixa de áudio")));
    }
}
