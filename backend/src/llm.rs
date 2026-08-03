//! R34 — o LLM, e a única coisa que ele tem permissão de fazer.
//!
//! ## A regra que isto NÃO derruba
//!
//! O §18 é o pilar mais citado deste projeto: **não mentir com cara de
//! metadado**. Ele foi aplicado duas vezes pra recusar geração de texto — na
//! trivia (§32) e na retrospectiva (§40) — e as duas recusas continuam de pé:
//!
//! > **Fato sobre filme, nunca.** Trivia inventada sobre um filme que alguém ama
//! > continua sendo pior que nenhuma trivia.
//!
//! O que a decisão 2.3 do `IDEIAS.md` abriu é outra coisa: **conteúdo
//! editorial**. O guia da semana é uma revista, e uma revista tem ensaio.
//!
//! ## A ressalva, e ela é o desenho inteiro
//!
//! > *"O sistema manda os fatos, o LLM escreve a costura."*
//!
//! A lista de filmes, os anos, os diretores e os países saem do **banco** — são
//! verdade. O modelo recebe essa lista pronta e escreve o texto em volta. Ele
//! nunca é perguntado *"quais filmes de terror existem?"*, porque a resposta a
//! essa pergunta é exatamente o tipo de coisa que ele inventaria com confiança.
//!
//! O `prompt` desta casa diz isso explicitamente ao modelo, e o `sistema` repete:
//! não acrescente filme, não invente ano, não cite o que não está na lista.
//!
//! ## Sem chave é um estado normal
//!
//! Não há chave do Groq configurada neste servidor hoje. Isso **não é uma
//! falha**: sem ela o guia mostra o tema e os filmes — que são fato — e omite o
//! ensaio. É o §24 (linha vazia some) e o §18 (não inventar) na mesma decisão.
//!
//! O dia em que a chave existir, ele liga sozinho.

use serde::Deserialize;

/// Quanto tempo esperar por um ensaio.
///
/// Generoso porque isto roda **fora** do caminho da tela: o guia serve a capa
/// sem o ensaio e o texto aparece na visita seguinte. Um timeout curto aqui só
/// produziria ensaios pela metade.
const ESPERA: std::time::Duration = std::time::Duration::from_secs(45);

/// O teto do que o modelo pode escrever.
///
/// Trezentos tokens são uns dois parágrafos. Uma capa de revista não é um
/// artigo, e um teto baixo é a forma mais barata de o texto não virar enchimento.
const TETO: u32 = 320;

#[derive(Debug, Clone)]
pub struct Llm {
    chave: String,
    pub modelo: String,
    http: reqwest::Client,
}

#[derive(Deserialize)]
struct Resposta {
    choices: Vec<Escolha>,
}

#[derive(Deserialize)]
struct Escolha {
    message: Mensagem,
}

#[derive(Deserialize)]
struct Mensagem {
    content: String,
}

impl Llm {
    /// `None` quando não há chave — e quem chama trata isso como ausência de
    /// ensaio, não como erro.
    pub fn novo(cfg: &crate::config::Config) -> Option<Self> {
        let chave = cfg.groq_api_key.clone()?;
        Some(Self {
            chave,
            modelo: cfg.groq_model.clone(),
            http: reqwest::Client::builder()
                .timeout(ESPERA)
                .build()
                .unwrap_or_default(),
        })
    }

    /// Escreve a costura em volta de fatos que já vieram prontos.
    ///
    /// `sistema` diz o que ele é; `fatos` é o material, e vem do banco. O modelo
    /// não busca nada — ele **redige**.
    pub async fn costurar(&self, sistema: &str, fatos: &str) -> anyhow::Result<String> {
        let corpo = serde_json::json!({
            "model": self.modelo,
            // Baixa, e não zero: zero produz o mesmo texto sempre, e a capa de
            // uma revista semanal que repete a redação parece defeito. Alta
            // produziria floreio, que é onde o modelo começa a inventar fato.
            "temperature": 0.6,
            "max_tokens": TETO,
            "messages": [
                { "role": "system", "content": sistema },
                { "role": "user", "content": fatos },
            ],
        });

        let r = self
            .http
            .post("https://api.groq.com/openai/v1/chat/completions")
            .bearer_auth(&self.chave)
            .json(&corpo)
            .send()
            .await?;

        if !r.status().is_success() {
            let status = r.status();
            let corpo = r.text().await.unwrap_or_default();
            anyhow::bail!("groq respondeu {status}: {}", corpo.chars().take(200).collect::<String>());
        }

        let resposta: Resposta = r.json().await?;
        let texto = resposta
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content.trim().to_string())
            .unwrap_or_default();

        if texto.is_empty() {
            anyhow::bail!("groq devolveu texto vazio");
        }
        Ok(texto)
    }
}

/// O que o modelo é, e o que ele não pode fazer.
///
/// Escrito em português porque o texto sai em português, e um sistema em inglês
/// pedindo saída em português é uma tradução a mais pra dar errado.
///
/// As três proibições são a ressalva da decisão 2.3 dita ao modelo. Elas não
/// substituem a arquitetura — os fatos já chegam prontos, então ele não teria de
/// onde buscar —, mas fecham o caminho de ele **completar** a lista de memória,
/// que é o jeito pelo qual isto daria errado.
pub const SISTEMA: &str = "\
Você escreve a capa de uma revista de cinema de uma videolocadora pessoal.

REGRAS ABSOLUTAS sobre fatos:
- Use SOMENTE os filmes, anos, diretores e países da lista que vem a seguir.
- NÃO acrescente nenhum filme que não esteja na lista, mesmo que caiba no tema.
- NÃO invente ano, diretor, prêmio, bilheteria nem curiosidade.
- Se você não sabe algo, não diga.

O QUE O TEXTO TEM QUE FAZER:
- Partir de uma OBSERVAÇÃO sobre o conjunto, usando a seção 'o que estes filmes \
têm em comum': a distância entre os anos, uma década que concentra, um país que \
aparece mais, dois filmes que conversam entre si.
- Citar 2 ou 3 filmes pelo nome, e dizer o que cada um faz ali — não listar.
- Ensinar alguma coisa sobre o recorte a quem lê.

PROIBIDO, porque é enchimento:
- Abrir com 'X é o tema da semana', 'esta semana' ou 'nesta seleção'.
- Frases como 'estão disponíveis para alugar', 'confira', 'não perca'.
- Adjetivo de resenha: imperdível, emocionante, cativante, jornada.
- Repetir a lista que já está na tela ao lado do texto.

FORMA: 2 parágrafos, português do Brasil, no máximo 130 palavras ao todo. Tom de \
quem trabalha na locadora e gosta de cinema: direto e específico. Sem título, \
sem lista, sem markdown.";
