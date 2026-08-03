# Odeon

Um servidor de mídia próprio. Não é um clone do Jellyfin — é a resposta ao que o
Jellyfin faz mal.

A tese: **não é um catálogo de arquivos, é uma biblioteca que te conhece.**

---

## Subir

Pré-requisitos: Docker. Só isso — Rust, Node e FFmpeg rodam dentro dos containers.

```bash
cp .env.example .env
```

Aponte `MEDIA_PATH` no `.env` pra onde estão suas mídias **no host**. Ela é
montada em `/media` dentro do container, **com escrita**.

> **A montagem tem escrita porque o "apagar do disco" da gaveta de gerenciar
> (R10) precisa dela.** Se você não quer que o servidor consiga tocar nos seus
> arquivos, acrescente `:ro` de volta nas três linhas `MEDIA_PATH` do
> `docker-compose.yml` e rode `docker compose up -d`. Nada quebra: o botão de
> apagar volta a nascer desabilitado explicando o porquê, porque
> `GET /api/storage` testa escrita de verdade em vez de deduzir da
> configuração.

```bash
docker compose up -d --build
```

A primeira subida compila o backend do zero dentro do container (alguns minutos).
Depois disso o `cargo watch` recompila só o que muda.

| serviço | onde |
|---|---|
| web | http://localhost:5174 |
| api | http://localhost:8080 |
| postgres | `localhost:5433` (`odeon` / `odeon`) |

Abra a web, crie o administrador, e vá na aba **pastas** pra escolher onde
estão suas mídias. Depois é **varrer**.

### Onde ficam os filmes

O `MEDIA_PATH` no `.env` é a **pasta raiz** que o servidor enxerga. Dentro dela
você cria quantas bibliotecas quiser pela interface — Filmes, Séries, Anime —
cada uma com seu tipo e seu identificador (TMDB, AniList, ou nenhum).

O container só enxerga o que está montado nele, então bibliotecas só podem
apontar pra dentro do `MEDIA_PATH`. Bibliotecas aninhadas são recusadas: um
arquivo pertence a uma biblioteca só, então a de dentro nasceria vazia.

**Disco em outra máquina?** Monte o compartilhamento no host primeiro (no macOS,
Finder → Ir → Conectar ao servidor → `smb://IP/pasta`) e aponte o `MEDIA_PATH`
pra `/Volumes/NOME`. Funciona, mas o vídeo trafega máquina → host → você. Rodar
o Odeon na máquina que tem o disco é sempre melhor.

```bash
docker compose logs -f api     # acompanhar o backend
docker compose exec db psql -U odeon    # abrir o banco
```

---

## O que já funciona

**M0 — espinha dorsal**

- Varredura recursiva do disco com `ffprobe` em cada arquivo
- Modelo de dados em grafo — sem tabela `movie` ou `tv_show`
- **Direct Play** com HTTP Range: seek funciona, sem transcodificar nada
- `play_event` como log cru + `playback_state` como cache de "continuar assistindo"
- Busca full-text + trigram

**M1 — identidade**

- Parser que usa **contexto de diretório** (`Severance/Season 2/S02E07.mkv` acha
  o título duas pastas acima) e entende **anime** (grupo de fansub, episódio
  absoluto, `[SubsPlease] Frieren - 12 [1080p]`)
- **TMDB** (filmes e séries) + **AniList** (anime, sem chave de API)
- **Score de confiança auditável**: toda tentativa vira linha em
  `match_candidate` com os motivos legíveis do score
- **Fila de revisão**: abaixo de 85% de confiança o matcher **não escreve nada**
  na obra — marca e pergunta. Acima de 85% entra sozinho.
- Artwork baixado e servido localmente, com **cor dominante** extraída do pôster
- Episódios criam `collection(série)` → `collection(temporada)` automaticamente

**M2 — o grafo**

- **Tags com namespace** (`mood:melancólico`, `genre:drama`) — os gêneros do
  TMDB/AniList entram sozinhos no match
- **Coleções recursivas** com CRUD: franquia → série → temporada, quantos níveis
  houver. Séries/temporadas o matcher cria; playlists e ordens são suas.
- **Ordens de exibição alternativas** (`watch_order`) reordenáveis — a "ordem
  Machete" é um caso de uso de primeira classe, não gambiarra
- **Relações obra↔obra**: `alternate_cut_of`, `sequel_of`, `remake_of`… lidas
  dos dois lados a partir de uma única aresta
- **Filtro composto** numa query: tags (`all`/`any`), faixa de ano, faixa de
  duração, coleção (subárvore inteira), estado de identificação, ordenação

**M3 — a alma**

- **Preview de seek**: folha de sprites gerada com ffmpeg, uma imagem por
  arquivo. Arrastar a timeline mostra o quadro daquele instante sem nenhuma
  requisição — o browser já tem a folha inteira.
- **Player próprio** (sem `<video controls>`): timeline com buffer, atalhos de
  teclado (espaço, ←/→, f, m), auto-hide, tela cheia
- **Cor dominante do pôster** extraída no match. Desde o redesenho ela fica na
  *arte* — halo do herói e do player, borda do pôster — e o amarelo responde
  pelo *sistema*: ação, foco, timeline, score (ver R1/R2)
- **Sync ao vivo entre aparelhos** via SSE: pausar no notebook move a TV. Cada
  device ignora o próprio eco pelo `device_id`.
- Design system com escala de espaço, tokens de movimento e
  `prefers-reduced-motion`

**M5 — curadoria**

- **Perfil de gosto derivado do `play_event`** — nada é declarado. O sinal forte
  é *terminar*, não *assistir*: largar aos 8 minutos conta como rejeição, e
  reassistir é o positivo mais forte que existe. Decai por recência (meia-vida
  de 60 dias).
- **Embeddings locais** (TF-IDF + hashing trick, 256 dim) em **pgvector** — sem
  API externa, sem mandar sua biblioteca pra terceiro
- **Contexto**: "tenho 40 minutos" é filtro duro; humor é `mood:` do M2
- **Todo item diz por quê** — "você costuma terminar palestra (100%)", "mas você
  larga reunião com frequência", "você parou faltando 72 min"
- **Perfil inspecionável** na própria tela: recomendação que não se deixa
  auditar é adivinhação
- Feedback explícito (♥ / ✕) pro que o comportamento não consegue inferir

**M6 — playback pesado**

- **Negociação auditável**: o player mostra um selo (Direct Play / Remux /
  Transcode) e, ao clicar, os motivos — *"o cliente não toca áudio em ac3"*.
  É a pergunta que o Jellyfin nunca responde direito.
- **Capacidade real do navegador**, perguntada a ele com `canPlayType` em vez de
  lista fixa. O Safari toca HEVC e recebe Direct Play; o resto não.
- **Aceleração detectada por encode de teste real** — `ffmpeg -encoders` lista o
  que foi compilado, não o que funciona. Cada encoder é testado no boot, e a
  recusa vem com motivo.
- **Transcode HLS sob demanda** com sessões: segmentos de 4s, keyframes
  alinhados, reaper que mata sessão ociosa e libera o disco
- **Legendas**: texto vira faixa WebVTT nativa (sem transcode); ASS/SSA e
  bitmaps (PGS/DVD) podem ser **queimados**, preservando o estilo original

### HTTPS

Desligado por padrão. Na tailnet o transporte já é criptografado, então o ganho
não é confidencialidade — é **contexto seguro** no navegador: Service Worker,
PWA offline, `crypto.subtle` e parte do Media Session API só existem sob HTTPS.

**Não use certificado auto-assinado.** Cada aparelho precisaria confiar numa CA
nova, e instalar CA em Android TV é um suplício. A Tailscale emite Let's Encrypt
de verdade:

```bash
tailscale cert --cert-file certs/cert.crt --key-file certs/cert.key odeon.SEU-TAILNET.ts.net
```

Depois, no `.env`:

```
ODEON_TLS_CERT=/certs/cert.crt
ODEON_TLS_KEY=/certs/cert.key
ODEON_HTTPS_ONLY=true
```

`ODEON_HTTPS_ONLY=true` faz a porta 8080 só redirecionar (308, que preserva
POST). Deixe `false` durante a migração.

**Os clientes se viram sozinhos.** Nos apps, basta digitar o host (`rog`): eles
tentam https antes de http. Na web, a API é deduzida da própria página — mesmo
host, mesmo esquema, porta 8443 sob HTTPS e 8080 sob HTTP. Não há URL fixa em
lugar nenhum pra atualizar.

Um detalhe que morde: **uma página HTTPS não pode chamar uma API HTTP** — o
navegador bloqueia como mixed content, e isso inclui o `<video>`. A web detecta
essa combinação e explica, em vez de parecer que o servidor caiu.

Para testar a camada TLS sem a tailnet: `./certs/dev-cert.sh` gera um
auto-assinado — todo navegador vai reclamar, e a TV não vai aceitar.

### TMDB precisa de chave

Grátis, mas precisa de conta: https://www.themoviedb.org/settings/api
Coloque em `TMDB_API_KEY` no `.env` e rode `docker compose up -d api`.
**Sem ela o Odeon ainda identifica anime** pelo AniList, que não pede chave.

**Elenco e equipe**

- **TMDB** (direção, roteiro, elenco, trilha) e **AniList** (staff e **dubladores
  com o personagem**) — em anime, quem dubla é informação de primeira classe
- **Deduplicação por `provider_key`**: "Villeneuve" é uma pessoa, não uma linha
  por filme. Sem isso, "tudo do Villeneuve" devolveria um filme.
- **Filtro da biblioteca por pessoa** — clicar num nome no detalhe filtra tudo
- **Afinidade por pessoa na curadoria**: "você terminou 2 obras com Shinichirou
  Watanabe" vira motivo de recomendação. Exige 2+ obras — com uma só, o elenco
  inteiro de um filme que você gostou viraria "gosto favorito".
- Retratos em cache local, igual ao artwork

**Autenticação**

- **Multiusuário** com senha em **Argon2id**, papéis `admin` e `user`
- **Sessões opacas revogáveis**, não JWT — deslogar o aparelho perdido é apagar
  uma linha. O token nunca toca o disco: guarda-se o SHA-256 dele.
- **Primeira execução guardada**: enquanto ninguém tem senha, `/api/auth/setup`
  responde; depois disso vira 403 pra sempre
- Operações de servidor (varrer, identificar, sprites, embeddings, bibliotecas)
  são só de admin; assistir e curar são de qualquer usuário
- Trocar a senha derruba as outras sessões

- **CORS pela regra do mesmo host**: o front em `http://rog:5174` falando com a
  API em `http://rog:8080` passa sozinho; `http://evil.com` não. Sem
  configuração, e sem quebrar quando o nome da máquina mudar.

Na primeira vez que abrir a web, ela pede pra criar o administrador.

## O que ainda não

HTTPS é opcional e desligado por padrão (ver abaixo).

---

## Roadmap

| | | |
|---|---|---|
| **M0** | Espinha dorsal | ✅ assistir um filme pelo seu servidor |
| **M1** | Identidade | ✅ TMDB + AniList, score auditável, fila de revisão, artwork |
| **M2** | O grafo | ✅ tags, coleções recursivas, relações, ordens de exibição, filtros |
| **M3** | A alma | ✅ preview de seek, player próprio, cor dominante, sync via SSE |
| **M4** | Clientes | ✅ Compose Multiplatform: celular, TV e iOS — os três compilam |
| **M5** | Curadoria | ✅ perfil de gosto, pgvector, contexto de tempo/humor, motivos |
| **M6** | Playback pesado | ✅ negociação auditável, HLS sob demanda, hwaccel real, legendas ASS |
| **R1** | Redesenho: o painel | ✅ marquise, herói "esta noite", motivos legíveis, manutenção fora da barra |
| **R2** | Redesenho: o player | ✅ sala escura, tempo de arquivo × tempo de sessão, selo do modo |
| **R3** | Redesenho: a biblioteca | ✅ série vira um cartão, contagem e paginação reais, +1 índice que faltava |
| **R4** | Redesenho: coleções | ✅ aba de curadoria, contagem recursiva, criar sem prompt(), arrastar pra ordenar |
| **R5** | Redesenho: revisão | ✅ pastas paginadas, um amarelo por decisão, a luz apagando ao tocar |
| **R6** | Canais ao vivo | ✅ IPTV via M3U + XMLTV, guia com grade, sessão compartilhada por canal |
| **R7** | Redesenho: a ficha da obra | ✅ vira cartaz com arte, ficha técnica e botão de assistir; edição atrás de um gesto |
| **R8** | Locadora (aba `experimentação`) | ✅ 600 caixas de VHS e DVD em estantes por gênero, em CSS 3D, com contracapa |
| **R18** | O título que o disco estragou | ✅ o sósia Unicode de `: ? \| /` desfeito no parser: 1.540 obras da biblioteca e o canal de clipes inteiro |
| **R18** | Guia de cinema | ✅ direção, elenco, trilha, gênero e década — cada nome com o que você tem e o que você fez com isso |
| **R17** | A arte da grade ao vivo | ✅ o XMLTV já mandava a foto e o Odeon a descartava: cobertura de 25% → 90%, e marquise da casa onde não há foto nenhuma |
| **R16** | Área de administração | ✅ pessoas, aparelhos, trabalhos e as quatro manutenções em ensaio-antes-de-executar; sete rotas ganharam tela |
| **R15** | Redesenho: para você | ✅ estados frio/morno/quente, calibragem por ♥/✕, motivo virou seção, e a marquise acende |
| **R13** | Ilha de transmissão + emissora própria | ✅ canais que o Odeon programa do acervo, linha do tempo com agulha viva, zapeamento e "ver desde o início" |
| **R12** | Vigia da grade + painel de saúde | ✅ guia ao vivo se repõe sozinho, lembretes sobrevivem à reimportação, e o que está torto aparece |
| **R11** | Locadora: a caixa como objeto (experimento) | ✅ voa da estante, gira arrastando, e a lombada abre a caixa antes de tocar |
| **R10** | Gerenciar a obra | ✅ cartão abre a ficha, ⋯ abre identificar à mão / corrigir parser / ignorar / apagar do disco |
| **R9** | A série vira dona da arte | ✅ sinopse, ids e pôster na coleção-série; episódio herda em vez de baixar (2,19 GB → 197 MB) |

Transcode ficou no M6 **de propósito**: é o maior sumidouro de complexidade do
projeto, e via Tailscale nos seus próprios aparelhos o Direct Play cobre quase
tudo. Adiá-lo é o que permitiu os outros cinco existirem. Ver
[docs/DESIGN.md](docs/DESIGN.md) pro raciocínio completo.

---

## Estrutura

```
backend/          Rust — axum + tokio + sqlx
  migrations/     schema (o coração do projeto está aqui)
  src/scanner/    walk + ffprobe + parser de nome
  src/metadata/   TMDB + AniList + score de confiança
  src/routes/     API HTTP
web/              React + TS + Vite
clients/          Kotlin Multiplatform — celular, TV e iOS (ver clients/README.md)
  shared/         modelos + Ktor + repositório, sem UI
  composeApp/     Compose MP: celular Android + iOS
  tv/             Android TV, foco por D-pad
docs/DESIGN.md    decisões de arquitetura e o porquê
```
