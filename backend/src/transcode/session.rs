//! Sessões de transcode.
//!
//! Cada sessão é um ffmpeg escrevendo segmentos HLS numa pasta própria. O
//! player consome a playlist; o servidor mata a sessão quando ninguém mais pede
//! segmento.
//!
//! **Seek dentro de transcode:** o ffmpeg produz do início ao fim, em ordem.
//! Pular pra frente do que já foi produzido significa recomeçar com outro
//! offset — ou seja, **outra sessão**. É o que o Jellyfin faz, e é o motivo de
//! `start_seconds` ser parte da identidade da sessão, não um parâmetro dela.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use uuid::Uuid;

use super::decide::{PlaybackPlan, StreamAction};
use super::hwaccel::Encoder;

/// Duração do segmento. 4s é o equilíbrio clássico: menor aumenta overhead de
/// requisição, maior atrasa o início da reprodução.
const SEGMENT_SECONDS: u32 = 4;

/// Sem pedido de segmento por este tempo, a sessão morre e o disco é liberado.
/// O player pede um segmento a cada ~4s, então 90s tolera pausa e buffer cheio.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

const REAPER_INTERVAL: Duration = Duration::from_secs(30);

/// Quanto esperar a playlist aparecer antes de desistir. O ffmpeg só escreve o
/// `.m3u8` depois de fechar o primeiro segmento.
const PLAYLIST_TIMEOUT: Duration = Duration::from_secs(25);

pub const PLAYLIST_NAME: &str = "index.m3u8";

/// O segmento de inicialização do fMP4 (R66). O ffmpeg o escreve no diretório
/// da sessão e o anuncia na playlist como `#EXT-X-MAP:URI="init.mp4"`.
pub const INIT_NAME: &str = "init.mp4";

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: Uuid,
    pub media_file_id: Uuid,
    pub start_seconds: f64,
    pub created_at: DateTime<Utc>,
    pub last_access: DateTime<Utc>,
    pub encoder: String,
    #[serde(flatten)]
    pub plan: PlaybackPlan,
}

struct Session {
    info: SessionInfo,
    dir: PathBuf,
    child: Child,
    /// Quem pediu a sessão.
    ///
    /// **Entrou na R26** (§42). Antes, `GET /api/hls/{session_id}/{arquivo}`
    /// servia os segmentos com o id da sessão como autorização inteira — um
    /// UUID é impalpável, mas "id não adivinhável" é capacidade, não permissão,
    /// e é exatamente a ressalva que o §9b já tinha feito sobre o `?token=`.
    /// Com um convidado no círculo, a diferença deixa de ser acadêmica.
    dono: Uuid,
}

pub struct SessionManager {
    sessions: Arc<Mutex<HashMap<Uuid, Session>>>,
    root: PathBuf,
    encoder: Encoder,
}

impl SessionManager {
    pub fn new(root: PathBuf, encoder: Encoder) -> Arc<Self> {
        let manager = Arc::new(Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            root,
            encoder,
        });
        manager.clone().spawn_reaper();
        manager
    }

    /// Mata sessões ociosas. Sem isto, cada seek deixaria um ffmpeg vivo
    /// comendo CPU e um diretório crescendo até o disco acabar.
    fn spawn_reaper(self: Arc<Self>) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(REAPER_INTERVAL).await;
                let now = Utc::now();
                let expired: Vec<Uuid> = {
                    let sessions = self.sessions.lock().await;
                    sessions
                        .iter()
                        .filter(|(_, session)| {
                            let idle = now - session.info.last_access;
                            idle.num_seconds() as u64 > IDLE_TIMEOUT.as_secs()
                        })
                        .map(|(id, _)| *id)
                        .collect()
                };
                for id in expired {
                    tracing::info!(%id, "sessão de transcode ociosa, encerrando");
                    self.stop(id).await;
                }
            }
        });
    }

    pub async fn start(
        &self,
        media_file_id: Uuid,
        source: &Path,
        plan: PlaybackPlan,
        start_seconds: f64,
        dono: Uuid,
        video_codec: Option<&str>,
    ) -> anyhow::Result<SessionInfo> {
        let id = Uuid::new_v4();
        let dir = self.root.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await?;

        let args = self.build_args(source, &dir, &plan, start_seconds, video_codec);
        tracing::info!(%id, ?args, "iniciando transcode");

        let child = Command::new("ffmpeg")
            .args(&args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let now = Utc::now();
        let info = SessionInfo {
            id,
            media_file_id,
            start_seconds,
            created_at: now,
            last_access: now,
            encoder: if plan.video == StreamAction::Encode {
                self.encoder.name.to_string()
            } else {
                "copy".to_string()
            },
            plan,
        };

        self.sessions.lock().await.insert(
            id,
            Session {
                info: info.clone(),
                dir,
                child,
                dono,
            },
        );

        Ok(info)
    }

    /// Abre (ou reaproveita) a transmissão de um canal.
    ///
    /// **Uma sessão por canal, compartilhada.** No transcode sob demanda cada
    /// pessoa está num ponto diferente do arquivo, então a sessão é por
    /// usuário. Ao vivo todos veem o mesmo instante — abrir um ffmpeg por
    /// espectador seria desperdício puro, e ainda multiplicaria a banda puxada
    /// do provedor.
    pub async fn live(&self, channel_id: Uuid, stream_url: &str) -> anyhow::Result<SessionInfo> {
        // Já existe transmissão deste canal? Entra nela.
        {
            let mut sessions = self.sessions.lock().await;
            if let Some((_, s)) = sessions.iter_mut().find(|(_, s)| s.info.media_file_id == channel_id) {
                s.info.last_access = Utc::now();
                return Ok(s.info.clone());
            }
        }

        let id = Uuid::new_v4();
        let dir = self.root.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await?;

        let args = self.build_live_args(stream_url, &dir);
        tracing::info!(%id, %channel_id, "abrindo canal ao vivo");

        let child = Command::new("ffmpeg")
            .args(&args)
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        let now = Utc::now();
        let info = SessionInfo {
            id,
            // O canal ocupa o lugar do arquivo: é a identidade da transmissão.
            media_file_id: channel_id,
            start_seconds: 0.0,
            created_at: now,
            last_access: now,
            encoder: "copy".to_string(),
            plan: PlaybackPlan {
                mode: super::decide::PlaybackMode::DirectStream,
                video: StreamAction::Copy,
                audio: StreamAction::Copy,
                target_height: None,
                burn_subtitle: None,
                // Ao vivo não há escolha de faixa: o `build_live_args` mapeia
                // `0:a:0?` e o provedor entrega uma faixa só.
                audio_track: None,
                reasons: vec![
                    "canal ao vivo: o provedor entrega MPEG-TS, que navegador nenhum toca"
                        .into(),
                    "vídeo e áudio são copiados bit a bit — só o container muda".into(),
                ],
            },
        };

        self.sessions
            .lock()
            .await
            .insert(id, Session { info: info.clone(), dir, child, dono: Uuid::nil() });

        Ok(info)
    }

    /// De quem é esta sessão. `None` quando ela não existe (ou já foi ceifada).
    ///
    /// `Uuid::nil()` é o dono das sessões de canal ao vivo: elas nascem da
    /// emissora (§25) e não de um pedido de pessoa, então não pertencem a
    /// ninguém em particular — e são da casa, que é o que o `hls_file` trata
    /// como aberto a morador.
    pub async fn dono(&self, id: Uuid) -> Option<Uuid> {
        self.sessions.lock().await.get(&id).map(|s| s.dono)
    }

    /// Argumentos do modo ao vivo.
    ///
    /// Duas diferenças que importam em relação ao sob demanda:
    ///
    /// - **janela deslizante** (`hls_list_size 6` + `delete_segments`): a
    ///   transmissão não acaba, e uma playlist que só cresce encheria o disco
    ///   até o fim dos tempos;
    /// - **sem `-hls_playlist_type event`**: aquilo declara um começo fixo, e
    ///   ao vivo não há começo — o player entra onde a transmissão está.
    fn build_live_args(&self, stream_url: &str, dir: &Path) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            // Reconecta sozinho: fonte ao vivo cai, e derrubar o canal na
            // primeira falha de rede seria pior que esperar alguns segundos.
            "-reconnect".into(),
            "1".into(),
            "-reconnect_streamed".into(),
            "1".into(),
            "-reconnect_delay_max".into(),
            "5".into(),
            "-i".into(),
            stream_url.to_string(),
        ];

        // Copia: o custo de recodificar N canais simultâneos não se paga, e o
        // problema aqui é de CONTAINER, não de codec.
        args.extend(
            [
                "-map", "0:v:0?", "-map", "0:a:0?",
                "-c", "copy",
                "-f", "hls",
                "-hls_time", &SEGMENT_SECONDS.to_string(),
                "-hls_list_size", "6",
                "-hls_flags", "delete_segments+independent_segments+omit_endlist",
                "-hls_segment_filename",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        args.push(dir.join("seg%05d.ts").to_string_lossy().to_string());
        args.push(dir.join(PLAYLIST_NAME).to_string_lossy().to_string());

        args
    }

    /// O filtro que reinsere os parâmetros do codec a cada keyframe (R50).
    ///
    /// ## O defeito, medido
    ///
    /// Num remux de mkv pra MPEG-TS com `-c:v copy`, os segmentos depois do
    /// primeiro saíam **sem SPS/PPS**. Medido nesta casa, na sessão do 007
    /// pt-BR: `seg00000` com 14 ocorrências, `seg00001` em diante com **zero**,
    /// e `ffmpeg` acusando `non-existing PPS 0 referenced` ao abrir cada um
    /// deles sozinho.
    ///
    /// O mkv guarda os parâmetros **fora de banda** (no `CodecPrivate`), e é o
    /// `*_mp4toannexb` que os traz pro fluxo. O ffmpeg 5.1 insere esse filtro
    /// sozinho no muxer de TS, mas — medido — só no começo do fluxo. Explícito,
    /// ele passa a inserir em cada segmento:
    ///
    /// | variante | seg0 | seg1 | seg2 |
    /// |---|---|---|---|
    /// | como estava | 2 SPS | **0** | **0** |
    /// | `-bsf:v h264_mp4toannexb` | 2 SPS | 2 SPS | 2 SPS |
    /// | `-mpegts_flags +resend_headers` | 2 SPS | **0** | **0** |
    ///
    /// ## Por quem isto passava despercebido
    ///
    /// O ExoPlayer guarda os parâmetros do primeiro segmento e segue tocando; o
    /// `AVPlayer` trata cada segmento como independente — que é o que a playlist
    /// manda fazer, com `#EXT-X-INDEPENDENT-SEGMENTS`. O sintoma no iOS era o
    /// pior tipo: centenas de MB baixados, nada bufferizado, **nenhum erro**.
    ///
    /// ## Por que por codec, e não sempre
    ///
    /// Aplicar o filtro errado **não degrada: mata**. Medido —
    /// `h264_mp4toannexb` num fluxo HEVC responde *"Codec 'hevc' is not
    /// supported by the bitstream filter"* e o ffmpeg nem inicia.
    ///
    /// E o HEVC do acervo não sofria do defeito: o x265 repete os parâmetros a
    /// cada keyframe por padrão, o x264 não. Mesmo assim o `hevc_mp4toannexb`
    /// entra — o filtro é idempotente (não duplica o que já está lá), e
    /// depender do padrão do encoder que gerou cada arquivo seria apostar num
    /// acervo que ninguém controla.
    fn bsf_annexb(video_codec: Option<&str>) -> Option<&'static str> {
        match video_codec.map(crate::transcode::decide::codec_static) {
            Some("h264") => Some("h264_mp4toannexb"),
            Some("hevc") => Some("hevc_mp4toannexb"),
            // AV1, VP9 e companhia não têm (nem precisam de) equivalente, e
            // MPEG-TS não os carrega. Sem filtro é a resposta certa.
            _ => None,
        }
    }

    /// **HEVC não vai em MPEG-TS** (R66).
    ///
    /// A *HLS Authoring Specification* da Apple é explícita: HEVC só é
    /// transportado em **fMP4**. MPEG-TS carrega HEVC pela norma do MPEG
    /// (`stream_type` 0x24), e o ffmpeg escreve isso sem reclamar — mas HLS
    /// nunca aceitou essa combinação, e é por isso que o `AVPlayer` não toca
    /// **nenhum** dos 5.374 arquivos HEVC deste acervo por essa via.
    ///
    /// ## O que foi medido, 18/08/2026, antes de mudar
    ///
    /// O relato era "nenhuma série toca, só tela preta", com o extrator do
    /// ExoPlayer repetindo `Unexpected start code prefix: 3211403 · 2162915 ·
    /// 3211491`. O segmento foi auditado byte a byte e **não tem defeito**:
    ///
    /// | conferido | resultado |
    /// |---|---|
    /// | bytes servidos × bytes em disco | idênticos |
    /// | `0x47` a cada 188, tamanho múltiplo de 188 | sim |
    /// | PAT/PMT | `0x24` HEVC com descritor `HEVC`, `0x0f` AAC |
    /// | pacotes com `payload_unit_start` que iniciam um PES | 192/192 e 73/73 |
    /// | quebras de `continuity_counter` | zero |
    /// | VPS/SPS/PPS por segmento | 8, 4, 4 |
    /// | decodificar o segmento sozinho | ffmpeg decodifica |
    ///
    /// E os três números do log do cliente são, em hexa, `0x31000B`,
    /// `0x210023` e `0x310063` — **exatamente os bytes do campo PTS** dos
    /// nossos PES (`31 00 …` no vídeo, `21 00 …` no áudio), que ficam no
    /// deslocamento 9. Ou seja: o parser dele recomeça a leitura 9 bytes
    /// adiante, num fluxo que está correto. Não há byte a consertar; há um
    /// contêiner a trocar.
    ///
    /// ## Por que só o HEVC muda
    ///
    /// H.264 em TS é a combinação mais antiga e mais testada do HLS, e é o que
    /// toca hoje — 887 dos 942 filmes. Trocar o contêiner dele seria mexer no
    /// que funciona pra consertar o que não funciona.
    ///
    /// ⚠️ E o `hevc_mp4toannexb` da R50 **sai** junto: ele existe pra pôr os
    /// NAL em Annex B, que é o que o TS quer. O fMP4 quer o contrário — NAL com
    /// prefixo de comprimento, e os parâmetros no segmento de inicialização.
    /// Aplicar o filtro aqui corromperia cada segmento.
    fn segmento_fmp4(plan: &PlaybackPlan, video_codec: Option<&str>) -> bool {
        plan.video == StreamAction::Copy
            && video_codec.map(crate::transcode::decide::codec_static) == Some("hevc")
    }

    fn build_args(
        &self,
        source: &Path,
        dir: &Path,
        plan: &PlaybackPlan,
        start_seconds: f64,
        video_codec: Option<&str>,
    ) -> Vec<String> {
        let fmp4 = Self::segmento_fmp4(plan, video_codec);
        let mut args: Vec<String> = vec!["-hide_banner".into(), "-loglevel".into(), "error".into()];

        // Só inicializa device de hardware quando o vídeo vai ser recodificado.
        if plan.video == StreamAction::Encode {
            args.extend(self.encoder.input_args.clone());
        }

        // `-ss` ANTES do `-i` é seek por keyframe: instantâneo. Depois do `-i`
        // seria preciso, mas decodificaria tudo até lá.
        if start_seconds > 0.0 {
            args.push("-ss".into());
            args.push(format!("{start_seconds:.3}"));
        }

        args.push("-i".into());
        args.push(source.to_string_lossy().to_string());

        // --- vídeo ---
        args.push("-map".into());
        args.push("0:v:0".into());
        match plan.video {
            StreamAction::Copy => {
                args.push("-c:v".into());
                args.push("copy".into());
                // R50 — os parâmetros voltam em cada segmento. **Só no TS**:
                // em fMP4 eles moram no `init.mp4` e o Annex B corromperia o
                // segmento (R66).
                if !fmp4 {
                    if let Some(bsf) = Self::bsf_annexb(video_codec) {
                        args.push("-bsf:v".into());
                        args.push(bsf.into());
                    }
                }
            }
            StreamAction::Encode => {
                let mut filters: Vec<String> = Vec::new();

                if let Some(index) = plan.burn_subtitle {
                    // O caminho vai dentro de aspas simples do filtergraph; os
                    // dois-pontos precisam de escape ou o parser corta o filtro.
                    let escaped = source
                        .to_string_lossy()
                        .replace('\\', "\\\\")
                        .replace('\'', "\\'")
                        .replace(':', "\\:");
                    filters.push(format!("subtitles='{escaped}':si={index}"));
                }

                if let Some(height) = plan.target_height {
                    // -2 mantém a proporção e garante largura par.
                    filters.push(format!("scale=-2:{height}"));
                }

                if !filters.is_empty() {
                    args.push("-vf".into());
                    args.push(filters.join(","));
                }

                args.push("-c:v".into());
                args.push(self.encoder.name.to_string());
                args.extend(self.encoder.output_args.clone());
                // Sai sempre em 8 bits (R58).
                //
                // **5.094 arquivos do acervo são 10 bits** — 5.060 hevc Main 10
                // e 34 h264 — e nenhum deles conseguia abrir sessão quando o
                // plano pedia recodificação de vídeo. O NVENC de H.264 não
                // codifica 10 bits (é limitação do H.264 no hardware, não do
                // ffmpeg), e a mensagem que ele dá esconde isso:
                //
                //     [h264_nvenc] No capable devices found
                //
                // Parece GPU ausente. Não é: o `nvidia-smi` lista a RTX 2060 e
                // um `testsrc` codifica na hora. O que falha é a checagem de
                // capacidade *por formato de pixel*, feita device a device — e
                // quando nenhum passa, o ffmpeg reporta como se não houvesse
                // device nenhum. Foi o que fez a caça começar pelo lado errado.
                //
                // O cliente via só `400 — a sessão não produziu playlist a
                // tempo`, 25 segundos depois de mandar tocar.
                //
                // Em fonte de 8 bits isto não faz nada, e é por isso que é
                // incondicional: o `libx264` aceitaria 10 bits, mas aí sairia
                // um High 10 que quase nenhum cliente decodifica — trocaria uma
                // sessão que não abre por uma que abre e não toca.
                args.push("-pix_fmt".into());
                args.push("yuv420p".into());
                // Keyframe a cada segmento: sem isto o segmentador do HLS
                // produz pedaços de duração irregular e o seek fica torto.
                args.push("-force_key_frames".into());
                args.push(format!("expr:gte(t,n_forced*{SEGMENT_SECONDS})"));
            }
        }

        // --- áudio ---
        //
        // A faixa vem do plano. Era `0:a:0?` fixo, e num arquivo dual audio isso
        // punha uma faixa só na playlist — o player oferece o que está na
        // playlist, não o que está no arquivo, então o botão de áudio não tinha
        // o que mostrar. O `?` continua tolerando arquivo mudo; o índice já foi
        // conferido contra o arquivo em `audio::escolher`, então ele não é um
        // caminho pra sessão silenciosa.
        args.push("-map".into());
        args.push(format!("0:a:{}?", plan.audio_track.unwrap_or(0)));
        match plan.audio {
            StreamAction::Copy => {
                args.push("-c:a".into());
                args.push("copy".into());
            }
            StreamAction::Encode => {
                args.extend(
                    ["-c:a", "aac", "-b:a", "192k", "-ac", "2"]
                        .iter()
                        .map(|s| s.to_string()),
                );
            }
        }

        // --- saída HLS ---
        args.extend(
            [
                "-f",
                "hls",
                "-hls_time",
                &SEGMENT_SECONDS.to_string(),
                "-hls_list_size",
                "0",
                // `event` deixa a playlist só crescer: o player pode voltar pra
                // qualquer ponto já produzido sem sessão nova.
                "-hls_playlist_type",
                "event",
                "-hls_flags",
                "independent_segments",
            ]
            .iter()
            .map(|s| s.to_string()),
        );

        // R66 — o contêiner do segmento. `init.mp4` guarda os parâmetros do
        // codec uma vez, e a playlist o anuncia num `#EXT-X-MAP`.
        if fmp4 {
            args.extend(
                [
                    "-hls_segment_type",
                    "fmp4",
                    "-hls_fmp4_init_filename",
                    INIT_NAME,
                    // ⚠️ `hvc1`, e não o `hev1` que o ffmpeg escolhe sozinho.
                    //
                    // São os dois nomes da mesma amostra de HEVC no MP4, e a
                    // diferença é onde os parâmetros do codec podem estar:
                    // `hev1` os aceita dentro do fluxo, `hvc1` exige que
                    // estejam na descrição da amostra — ou seja, no `init.mp4`.
                    // A especificação de HLS da Apple **só admite `hvc1`**, e o
                    // `AVPlayer` recusa o outro. Medido aqui: sem esta linha o
                    // segmento sai com `codec_tag_string=hev1`.
                    "-tag:v",
                    "hvc1",
                ]
                .iter()
                .map(|s| s.to_string()),
            );
        }

        args.push("-hls_segment_filename".into());
        let padrao = if fmp4 { "seg%05d.m4s" } else { "seg%05d.ts" };
        args.push(dir.join(padrao).to_string_lossy().to_string());
        args.push(dir.join(PLAYLIST_NAME).to_string_lossy().to_string());

        args
    }

    /// Caminho de um arquivo da sessão, marcando acesso. `None` se a sessão não
    /// existe — o que também impede pedir arquivo de fora do diretório dela.
    pub async fn resolve(&self, id: Uuid, filename: &str) -> Option<PathBuf> {
        // Nada de `..` ou barra: o nome vem da URL.
        if filename.contains('/') || filename.contains("..") {
            return None;
        }
        let mut sessions = self.sessions.lock().await;
        let session = sessions.get_mut(&id)?;
        session.info.last_access = Utc::now();
        Some(session.dir.join(filename))
    }

    /// Espera a playlist existir. O ffmpeg só a escreve após o 1º segmento.
    pub async fn wait_for_playlist(&self, id: Uuid) -> Option<PathBuf> {
        let deadline = tokio::time::Instant::now() + PLAYLIST_TIMEOUT;
        loop {
            let path = self.resolve(id, PLAYLIST_NAME).await?;
            if tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return Some(path);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn stop(&self, id: Uuid) {
        let session = self.sessions.lock().await.remove(&id);
        if let Some(mut session) = session {
            let _ = session.child.kill().await;
            if let Err(e) = tokio::fs::remove_dir_all(&session.dir).await {
                tracing::warn!(%id, error = %e, "não consegui limpar a pasta da sessão");
            }
        }
    }

    pub async fn list(&self) -> Vec<SessionInfo> {
        self.sessions
            .lock()
            .await
            .values()
            .map(|session| session.info.clone())
            .collect()
    }

    pub fn encoder_name(&self) -> &'static str {
        self.encoder.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcode::decide::PlaybackMode;
    use crate::transcode::hwaccel::EncoderKind;

    fn manager() -> SessionManager {
        SessionManager {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            root: PathBuf::from("/tmp/odeon-test"),
            encoder: Encoder {
                name: "libx264",
                label: "teste",
                kind: EncoderKind::Software,
                input_args: vec![],
                output_args: vec!["-crf".into(), "21".into()],
            },
        }
    }

    fn plan(video: StreamAction, audio: StreamAction) -> PlaybackPlan {
        PlaybackPlan {
            mode: PlaybackMode::Transcode,
            video,
            audio,
            target_height: None,
            burn_subtitle: None,
            audio_track: None,
            reasons: vec![],
        }
    }

    /// **A R50.** Sem isto, todo segmento depois do primeiro sai sem SPS/PPS e
    /// o `AVPlayer` baixa centenas de MB sem bufferizar nada — sem erro.
    #[test]
    fn o_remux_reinsere_os_parametros_em_cada_segmento() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Copy, StreamAction::Copy),
                0.0,
                Some("h264"),
            )
            .join(" ");
        assert!(args.contains("-bsf:v h264_mp4toannexb"), "veio {args}");
    }

    /// **A R66.** HEVC em MPEG-TS não é HLS válido, e era o que a sessão de
    /// toda série produzia — o extrator do ExoPlayer não formava um quadro e o
    /// player ficava em `BUFFERING` pra sempre, sem erro.
    #[test]
    fn hevc_copiado_sai_em_fmp4() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Copy, StreamAction::Encode),
                0.0,
                Some("hevc"),
            )
            .join(" ");
        assert!(args.contains("-hls_segment_type fmp4"), "veio {args}");
        assert!(args.contains("init.mp4"), "veio {args}");
        assert!(args.contains("seg%05d.m4s"), "veio {args}");
        // A especificação de HLS só admite `hvc1`; o padrão do ffmpeg é `hev1`.
        assert!(args.contains("-tag:v hvc1"), "veio {args}");
    }

    /// ⚠️ O Annex B é do TS. Em fMP4 ele corromperia cada segmento — os
    /// parâmetros moram no `init.mp4` e os NAL vão com prefixo de comprimento.
    #[test]
    fn fmp4_nao_leva_o_filtro_annexb() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Copy, StreamAction::Encode),
                0.0,
                Some("hevc"),
            )
            .join(" ");
        assert!(!args.contains("-bsf:v"), "veio {args}");
    }

    /// H.264 em TS é a combinação mais testada do HLS e é a que toca hoje.
    /// Trocar o contêiner dela seria mexer no que funciona.
    #[test]
    fn h264_continua_em_mpegts_com_annexb() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Copy, StreamAction::Copy),
                0.0,
                Some("h264"),
            )
            .join(" ");
        assert!(!args.contains("fmp4"), "veio {args}");
        assert!(args.contains("seg%05d.ts"), "veio {args}");
        assert!(args.contains("-bsf:v h264_mp4toannexb"), "veio {args}");
    }

    /// Recodificar sempre sai em H.264 — o encoder é `h264_*` —, então o TS
    /// vale mesmo quando a **fonte** é HEVC.
    #[test]
    fn recodificar_hevc_sai_em_ts_porque_o_destino_e_h264() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Encode, StreamAction::Encode),
                0.0,
                Some("hevc"),
            )
            .join(" ");
        assert!(!args.contains("fmp4"), "veio {args}");
        assert!(args.contains("seg%05d.ts"), "veio {args}");
    }

    /// **A R58.** 5.094 arquivos do acervo são 10 bits, e o NVENC de H.264 não
    /// codifica 10 bits — a sessão morria antes da primeira playlist, com uma
    /// mensagem que culpa a GPU (`No capable devices found`).
    #[test]
    fn recodificar_sai_em_8_bits() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Encode, StreamAction::Encode),
                0.0,
                Some("hevc"),
            )
            .join(" ");
        assert!(args.contains("-pix_fmt yuv420p"), "veio {args}");
    }

    /// Copiar é copiar: mexer no formato de pixel de um fluxo que não vai ser
    /// decodificado faria o ffmpeg recusar a sessão inteira.
    #[test]
    fn copiar_nao_declara_formato_de_pixel() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Copy, StreamAction::Encode),
                0.0,
                Some("hevc"),
            )
            .join(" ");
        assert!(!args.contains("-pix_fmt"), "veio {args}");
    }

    /// O filtro é escolhido pelo codec da fonte. **Errar aqui não degrada:
    /// mata** — `h264_mp4toannexb` num fluxo HEVC impede o ffmpeg de iniciar.
    #[test]
    fn o_filtro_segue_o_codec_da_fonte() {
        assert_eq!(
            SessionManager::bsf_annexb(Some("hevc")),
            Some("hevc_mp4toannexb")
        );
        assert_eq!(
            SessionManager::bsf_annexb(Some("h264")),
            Some("h264_mp4toannexb")
        );
        // apelidos que o `codec_static` normaliza
        assert_eq!(
            SessionManager::bsf_annexb(Some("avc1")),
            Some("h264_mp4toannexb")
        );
        assert_eq!(
            SessionManager::bsf_annexb(Some("h265")),
            Some("hevc_mp4toannexb")
        );
        // e o que não tem equivalente fica sem filtro, em vez de ganhar um errado
        for codec in ["av1", "vp9", "mpeg2video", "desconhecido"] {
            assert_eq!(SessionManager::bsf_annexb(Some(codec)), None, "{codec}");
        }
        assert_eq!(SessionManager::bsf_annexb(None), None);
    }

    /// Quando o vídeo é **recodificado**, o encoder já entrega annexb — pôr o
    /// filtro ali seria trabalho a mais no melhor caso e erro no pior.
    #[test]
    fn quem_recodifica_nao_leva_o_filtro() {
        let args = manager()
            .build_args(
                Path::new("/media/f.mkv"),
                Path::new("/cache/s"),
                &plan(StreamAction::Encode, StreamAction::Copy),
                0.0,
                Some("h264"),
            )
            .join(" ");
        assert!(!args.contains("mp4toannexb"), "veio {args}");
    }

    #[test]
    fn remux_nao_recodifica_nada() {
        let args = manager().build_args(
            Path::new("/media/f.mkv"),
            Path::new("/cache/s"),
            &plan(StreamAction::Copy, StreamAction::Copy),
            0.0,
        
            Some("h264"),
        );
        let joined = args.join(" ");
        assert!(joined.contains("-c:v copy"));
        assert!(joined.contains("-c:a copy"));
        assert!(!joined.contains("libx264"));
    }

    #[test]
    fn seek_vai_antes_do_input() {
        let args = manager().build_args(
            Path::new("/media/f.mkv"),
            Path::new("/cache/s"),
            &plan(StreamAction::Copy, StreamAction::Copy),
            42.5,
        
            Some("h264"),
        );
        let ss = args.iter().position(|a| a == "-ss").unwrap();
        let input = args.iter().position(|a| a == "-i").unwrap();
        assert!(ss < input, "-ss precisa vir antes do -i pra seek rápido");
    }

    #[test]
    fn downscale_mantem_proporcao() {
        let mut p = plan(StreamAction::Encode, StreamAction::Copy);
        p.target_height = Some(720);
        let args = manager().build_args(
            Path::new("/media/f.mkv"),
            Path::new("/cache/s"),
            &p,
            0.0,
        
            Some("h264"),
        );
        assert!(args.join(" ").contains("scale=-2:720"));
    }

    #[test]
    fn caminho_da_legenda_e_escapado() {
        let mut p = plan(StreamAction::Encode, StreamAction::Copy);
        p.burn_subtitle = Some(1);
        let args = manager().build_args(
            Path::new("/media/Serie: Piloto.mkv"),
            Path::new("/cache/s"),
            &p,
            0.0,
        
            Some("h264"),
        );
        let filter = args.join(" ");
        assert!(filter.contains("subtitles="));
        // dois-pontos sem escape cortaria o filtergraph no meio do caminho
        assert!(filter.contains("Serie\\:"));
        assert!(filter.contains("si=1"));
    }

    #[test]
    fn audio_opcional_nao_quebra_arquivo_mudo() {
        let args = manager().build_args(
            Path::new("/media/f.mkv"),
            Path::new("/cache/s"),
            &plan(StreamAction::Copy, StreamAction::Copy),
            0.0,
        
            Some("h264"),
        );
        assert!(args.contains(&"0:a:0?".to_string()));
    }

    #[test]
    fn faixa_escolhida_e_a_que_entra_na_playlist() {
        let mut p = plan(StreamAction::Copy, StreamAction::Copy);
        p.audio_track = Some(1);
        let args = manager().build_args(
            Path::new("/media/dual.mkv"),
            Path::new("/cache/s"),
            &p,
            0.0,
        
            Some("h264"),
        );
        assert!(args.contains(&"0:a:1?".to_string()));
        assert!(!args.contains(&"0:a:0?".to_string()));
    }

    #[tokio::test]
    async fn resolve_recusa_travessia_de_caminho() {
        let m = manager();
        assert!(m.resolve(Uuid::new_v4(), "../../etc/passwd").await.is_none());
        assert!(m.resolve(Uuid::new_v4(), "a/b").await.is_none());
    }
}
