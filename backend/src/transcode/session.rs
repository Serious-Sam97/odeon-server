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
    ) -> anyhow::Result<SessionInfo> {
        let id = Uuid::new_v4();
        let dir = self.root.join(id.to_string());
        tokio::fs::create_dir_all(&dir).await?;

        let args = self.build_args(source, &dir, &plan, start_seconds);
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
            .insert(id, Session { info: info.clone(), dir, child });

        Ok(info)
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

    fn build_args(
        &self,
        source: &Path,
        dir: &Path,
        plan: &PlaybackPlan,
        start_seconds: f64,
    ) -> Vec<String> {
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
                // Keyframe a cada segmento: sem isto o segmentador do HLS
                // produz pedaços de duração irregular e o seek fica torto.
                args.push("-force_key_frames".into());
                args.push(format!("expr:gte(t,n_forced*{SEGMENT_SECONDS})"));
            }
        }

        // --- áudio ---
        args.push("-map".into());
        args.push("0:a:0?".into()); // o `?` tolera arquivo sem faixa de áudio
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
                "-hls_segment_filename",
            ]
            .iter()
            .map(|s| s.to_string()),
        );
        args.push(dir.join("seg%05d.ts").to_string_lossy().to_string());
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
            reasons: vec![],
        }
    }

    #[test]
    fn remux_nao_recodifica_nada() {
        let args = manager().build_args(
            Path::new("/media/f.mkv"),
            Path::new("/cache/s"),
            &plan(StreamAction::Copy, StreamAction::Copy),
            0.0,
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
        );
        assert!(args.contains(&"0:a:0?".to_string()));
    }

    #[tokio::test]
    async fn resolve_recusa_travessia_de_caminho() {
        let m = manager();
        assert!(m.resolve(Uuid::new_v4(), "../../etc/passwd").await.is_none());
        assert!(m.resolve(Uuid::new_v4(), "a/b").await.is_none());
    }
}
