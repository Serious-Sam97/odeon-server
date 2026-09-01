//! Detecção de aceleração por hardware — por **encode de teste real**.
//!
//! `ffmpeg -encoders` lista o que foi compilado, não o que funciona. Neste
//! container o `h264_nvenc` aparece na lista e morre com "Cannot load
//! libcuda.so.1" na hora do play. Esse é exatamente o bug que faz o Jellyfin
//! oferecer aceleração que quebra no meio do filme.
//!
//! Aqui cada encoder é testado codificando um quadro sintético no boot. O que
//! não codificar não entra na lista. E o resultado de cada tentativa fica
//! guardado com o motivo da falha — mesma regra de auditabilidade do M1 e M5.

use serde::Serialize;
use tokio::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EncoderKind {
    Hardware,
    Software,
}

#[derive(Debug, Clone, Serialize)]
pub struct Encoder {
    pub name: &'static str,
    pub label: &'static str,
    pub kind: EncoderKind,
    /// Argumentos que vão ANTES do `-i` (inicialização de device).
    pub input_args: Vec<String>,
    /// Argumentos de qualidade/preset, depois do `-c:v`.
    pub output_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProbeResult {
    pub encoder: &'static str,
    pub label: &'static str,
    pub kind: EncoderKind,
    pub usable: bool,
    /// Por que falhou. Vazio quando funcionou.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Capabilities {
    /// O escolhido: o primeiro hardware que passou, senão software.
    pub chosen: Encoder,
    /// Tudo que foi tentado, com o motivo de cada recusa.
    pub probed: Vec<ProbeResult>,
    pub has_hardware: bool,
}

/// Ordem de preferência. NVENC primeiro porque o alvo do projeto é o ROG com
/// NVIDIA; VideoToolbox pra quando rodar direto no Mac; VAAPI pro Intel/AMD.
fn candidates() -> Vec<Encoder> {
    vec![
        Encoder {
            name: "h264_nvenc",
            label: "NVIDIA NVENC",
            kind: EncoderKind::Hardware,
            input_args: vec![],
            // p4 = equilíbrio; vbr com cq deixa a qualidade constante e o
            // bitrate variar, que é o certo pra biblioteca heterogênea.
            //
            // `-forced-idr 1` (R58) é o que faz o `-force_key_frames` do
            // `build_args` valer alguma coisa aqui. Sem ele o NVENC atende o
            // pedido com um **I-frame comum**, que não é ponto de entrada — o
            // segmentador do HLS só corta em IDR, então ignorava as marcas e
            // seguia o GOP do encoder. Medido em 17/08/2026, no mesmo clipe de
            // 60 s: sem a opção, 6 segmentos de 10,000 s; com ela, 15 de
            // 4,000 s, que é o `SEGMENT_SECONDS` pedido.
            //
            // O sintoma não era erro nenhum: só ~10 s pra o vídeo começar e um
            // seek grosseiro. É específico do NVENC — o `libx264` já obedece —
            // e por isso mora no encoder, não no `build_args`.
            output_args: vec![
                "-preset".into(), "p4".into(),
                "-rc".into(), "vbr".into(),
                "-cq".into(), "23".into(),
                "-forced-idr".into(), "1".into(),
            ],
        },
        Encoder {
            name: "h264_qsv",
            label: "Intel Quick Sync",
            kind: EncoderKind::Hardware,
            input_args: vec![],
            output_args: vec![
                "-preset".into(), "medium".into(),
                "-global_quality".into(), "23".into(),
            ],
        },
        Encoder {
            name: "h264_vaapi",
            label: "VAAPI (Intel/AMD)",
            kind: EncoderKind::Hardware,
            input_args: vec![
                "-vaapi_device".into(), "/dev/dri/renderD128".into(),
            ],
            output_args: vec!["-qp".into(), "23".into()],
        },
        Encoder {
            name: "h264_videotoolbox",
            label: "Apple VideoToolbox",
            kind: EncoderKind::Hardware,
            input_args: vec![],
            output_args: vec!["-q:v".into(), "55".into()],
        },
        Encoder {
            name: "libx264",
            label: "libx264 (software)",
            kind: EncoderKind::Software,
            input_args: vec![],
            // veryfast é o ponto onde o transcode acompanha a reprodução em CPU
            // modesta. Mais lento que isso e o player fica esperando.
            output_args: vec![
                "-preset".into(), "veryfast".into(),
                "-crf".into(), "21".into(),
            ],
        },
    ]
}

/// Tenta codificar um quadro sintético. É o único teste que não mente.
async fn probe_one(encoder: &Encoder) -> ProbeResult {
    // VAAPI precisa do filtro de upload; sem ele o teste falha por motivo
    // errado e a gente descartaria um encoder que funciona.
    let filter = if encoder.name == "h264_vaapi" {
        vec!["-vf".to_string(), "format=nv12,hwupload".to_string()]
    } else {
        vec![]
    };

    let mut command = Command::new("ffmpeg");
    command.args(["-hide_banner", "-loglevel", "error"]);
    command.args(&encoder.input_args);
    command.args([
        "-f", "lavfi", "-i", "testsrc=size=320x240:rate=25:duration=0.2",
    ]);
    command.args(&filter);
    command.args(["-c:v", encoder.name]);
    command.args(&encoder.output_args);
    command.args(["-frames:v", "5", "-f", "null", "-"]);

    match command.output().await {
        Ok(output) if output.status.success() => ProbeResult {
            encoder: encoder.name,
            label: encoder.label,
            kind: encoder.kind,
            usable: true,
            error: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // A primeira linha de erro do ffmpeg é a que diz a causa real.
            let reason = stderr
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("falhou sem mensagem")
                .trim()
                .to_string();
            ProbeResult {
                encoder: encoder.name,
                label: encoder.label,
                kind: encoder.kind,
                usable: false,
                error: Some(reason),
            }
        }
        Err(e) => ProbeResult {
            encoder: encoder.name,
            label: encoder.label,
            kind: encoder.kind,
            usable: false,
            error: Some(format!("não consegui executar o ffmpeg: {e}")),
        },
    }
}

/// Roda no boot. Custa alguns segundos uma vez, e evita descobrir no meio de um
/// filme que a aceleração não existe.
pub async fn detect() -> Capabilities {
    let all = candidates();
    let mut probed = Vec::with_capacity(all.len());

    for encoder in &all {
        let result = probe_one(encoder).await;
        if result.usable {
            tracing::info!(encoder = encoder.name, "encoder disponível");
        } else {
            tracing::debug!(
                encoder = encoder.name,
                motivo = result.error.as_deref().unwrap_or(""),
                "encoder indisponível",
            );
        }
        probed.push(result);
    }

    let chosen = all
        .iter()
        .find(|encoder| {
            probed
                .iter()
                .any(|result| result.encoder == encoder.name && result.usable)
        })
        .cloned()
        // Se nem o libx264 passar, algo está muito errado — mas devolver o
        // software mesmo assim dá uma mensagem de erro melhor do que entrar
        // em pânico no boot.
        .unwrap_or_else(|| all.last().cloned().expect("sempre há libx264"));

    let has_hardware = chosen.kind == EncoderKind::Hardware;

    tracing::info!(
        escolhido = chosen.name,
        hardware = has_hardware,
        "aceleração de vídeo definida"
    );

    Capabilities {
        chosen,
        probed,
        has_hardware,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn software_e_sempre_o_ultimo_candidato() {
        let all = candidates();
        assert_eq!(all.last().unwrap().kind, EncoderKind::Software);
        assert!(all[..all.len() - 1]
            .iter()
            .all(|e| e.kind == EncoderKind::Hardware));
    }

    /// Sem `-forced-idr`, o `-force_key_frames` que o `build_args` manda é
    /// atendido com I-frame comum e o segmentador do HLS o ignora.
    #[test]
    fn o_nvenc_forca_idr_de_verdade() {
        let nvenc = candidates()
            .into_iter()
            .find(|e| e.name == "h264_nvenc")
            .unwrap();
        assert!(nvenc.output_args.iter().any(|a| a == "-forced-idr"));
    }

    #[test]
    fn vaapi_declara_o_device() {
        let vaapi = candidates()
            .into_iter()
            .find(|e| e.name == "h264_vaapi")
            .unwrap();
        assert!(vaapi.input_args.iter().any(|a| a.contains("renderD128")));
    }
}
