use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: String,
    pub database_url: String,
    /// Pastas que o container enxerga. A biblioteca só pode apontar pra
    /// dentro delas — é o que o navegador de pastas confere.
    pub media_roots: Vec<PathBuf>,
    pub cache_dir: PathBuf,
    pub artwork_dir: PathBuf,
    pub scrub_dir: PathBuf,
    /// Aceita chave v3 (hex) ou token v4 (JWT). Sem ela, filmes e séries não são
    /// identificados — o AniList (anime) não precisa de chave nenhuma.
    pub tmdb_api_key: Option<String>,
    /// A chave do Groq, pro conteúdo editorial do guia (R34).
    ///
    /// **Ausente é um estado normal**, não uma falha de configuração: sem ela o
    /// guia mostra o tema e os filmes — que são fato do banco — e **omite o
    /// ensaio**, em vez de inventar prosa. É o §18 aplicado ao texto gerado, e é
    /// a razão de o tipo ser `Option` em vez de o boot exigir a chave.
    pub groq_api_key: Option<String>,
    /// Qual modelo escreve. Mora em variável porque a lista do Groq muda mais
    /// rápido que este código, e trocar de modelo não devia exigir recompilar.
    pub groq_model: String,
    /// Origens extras aceitas pelo CORS, além de localhost e do próprio host.
    pub allowed_origins: Vec<String>,
    /// `ODEON_ALLOWED_ORIGINS=*` — escotilha de emergência, com aviso no boot.
    pub allow_any_origin: bool,
    /// Par de arquivos do certificado. Ausentes = só HTTP.
    pub tls_cert: Option<PathBuf>,
    pub tls_key: Option<PathBuf>,
    pub https_bind: String,
    /// Quando há TLS, a porta HTTP só redireciona em vez de servir.
    pub https_only: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let cache_dir = PathBuf::from(env_or("ODEON_CACHE_DIR", "/cache"));
        let origins_raw = env_or("ODEON_ALLOWED_ORIGINS", "");
        Ok(Self {
            bind: env_or("ODEON_BIND", "0.0.0.0:8080"),
            database_url: std::env::var("DATABASE_URL")
                .map_err(|_| anyhow::anyhow!("DATABASE_URL não definida"))?,
            media_roots: distinct_dirs(&env_or("ODEON_MEDIA_ROOTS", "/media")),
            artwork_dir: cache_dir.join("artwork"),
            scrub_dir: cache_dir.join("scrub"),
            cache_dir,
            tmdb_api_key: std::env::var("TMDB_API_KEY")
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
            groq_api_key: std::env::var("GROQ_API_KEY")
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty()),
            groq_model: env_or("GROQ_MODEL", "llama-3.3-70b-versatile"),
            tls_cert: std::env::var("ODEON_TLS_CERT").ok().filter(|v| !v.is_empty()).map(PathBuf::from),
            tls_key: std::env::var("ODEON_TLS_KEY").ok().filter(|v| !v.is_empty()).map(PathBuf::from),
            https_bind: env_or("ODEON_HTTPS_BIND", "0.0.0.0:8443"),
            https_only: env_or("ODEON_HTTPS_ONLY", "false") == "true",
            allow_any_origin: origins_raw.trim() == "*",
            allowed_origins: origins_raw
                .split(',')
                .map(|o| o.trim().trim_end_matches('/').to_string())
                .filter(|o| !o.is_empty() && o != "*")
                .collect(),
        })
    }
}

/// Raízes que existem e são realmente distintas.
///
/// Os mounts extras (`/media2`, `/media3`) apontam pro mesmo disco quando não
/// há disco extra — o compose não sabe omitir um volume condicionalmente. Sem
/// deduplicar, a interface ofereceria a mesma pasta três vezes.
///
/// A comparação é por (device, inode): dois bind mounts do mesmo diretório têm
/// caminhos diferentes mas o mesmo inode, então canonicalizar não bastaria.
fn distinct_dirs(raw: &str) -> Vec<PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let mut seen: Vec<(u64, u64)> = Vec::new();
    let mut out = Vec::new();

    for candidate in raw.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let path = PathBuf::from(candidate);
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        if !meta.is_dir() {
            continue;
        }
        let key = (meta.dev(), meta.ino());
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        out.push(path);
    }
    out
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
