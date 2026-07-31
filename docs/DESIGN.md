# Odeon — decisões de arquitetura

Documento vivo. Registra **por que** cada escolha foi feita, pra que daqui a seis
meses ninguém (inclusive eu) desfaça uma decisão boa por esquecer o motivo.

---

## 1. A tese

> Não é um catálogo de arquivos. É uma biblioteca que te conhece.

Quatro pilares, em ordem de importância:

1. **Modelo de dados aberto.** O Jellyfin é rígido: Filme / Série / Música e
   acabou. O Odeon é um grafo. Isso sozinho resolve anime, ordem alternativa de
   exibição, cortes do diretor, stand-up, documentário em partes.
2. **Identificação que pergunta.** O Jellyfin erra em silêncio. O Odeon tem
   score de confiança e uma fila de "não tenho certeza, me ajuda".
3. **Curadoria ativa.** "Tenho 40 minutos", "tô pra baixo", "vou assistir
   acompanhado".
4. **Interface com alma.** Cinematográfica, opinativa.

Se uma feature não serve a um desses quatro, ela não entra.

---

## 2. O que é difícil de verdade

O erro clássico é achar que o projeto é a UI. Não é.

| | dificuldade | valor pra este projeto |
|---|---|---|
| Catálogo, player, usuários | baixa | é a mesa, não o prato |
| **Matching de metadata** | média | **altíssimo** — é onde o Jellyfin é pior |
| **Transcode + negociação de codec** | brutal | **baixo** — ver §3 |
| Legenda ASS/SSA | alta | médio |

---

## 3. Por que transcode fica pro final

O acesso é por **Tailscale**, nos **meus próprios aparelhos**. Isso muda o
cálculo inteiro:

- a banda existe (rede local ou WireGuard direto);
- o conjunto de clientes é conhecido e pequeno;
- logo, **Direct Play é o caso comum, não a exceção**.

A matriz Direct Play → Direct Stream → Transcode é onde a maioria dos media
servers caseiros morre. Adiando isso pro M6, o M0 sai em dias e não em meses. O
custo é honesto e visível: se o navegador não souber o codec, o player diz isso
na cara em vez de fingir.

---

## 4. Postgres, não SQLite

Escolha do dono do projeto, e ela se paga:

- `tsvector` + `pg_trgm` juntos dão busca boa sem serviço externo;
- `pgvector` no M5 resolve curadoria semântica sem sair do banco;
- `LATERAL` deixa "obra + melhor arquivo + onde parei" em um round-trip;
- tudo em container, então roda igual no Mac e no ROG (CachyOS).

Custo aceito: um serviço a mais, contra o arquivo único do SQLite.

---

## 5. Tipos são TEXT + CHECK, não ENUM

`work.kind`, `match_state`, `event_type` — todos `text` com `CHECK`.

ENUM do Postgres não deixa remover nem reordenar valores, e este modelo vai
mudar muito nos próximos meses. `CHECK` se altera com um `ALTER TABLE`. Bônus:
some a fricção de mapear enum pra Rust via `sqlx::Type`.

---

## 6. FFmpeg como subprocesso, nunca binding

`ffprobe` e `ffmpeg` são chamados como processos filhos. Nada de `ffmpeg-next`
ou FFI com libav.

É o que o Jellyfin faz. Sobrevive a upgrade de FFmpeg, isola crash de codec do
processo do servidor, e evita meses de `unsafe` e build de C.

---

## 7. sqlx com queries em runtime, não macro

`sqlx::query_as::<_, T>(SQL)` em vez de `sqlx::query_as!`.

A macro checa o SQL em tempo de compilação — ótimo — mas exige um banco vivo
(ou `sqlx prepare` em dia) pra **compilar**. Dentro do Docker, com `cargo watch`
rodando, isso vira atrito constante. Trocamos checagem estática por fluidez.
Vale reavaliar quando o schema estabilizar.

---

## 8. O modelo de dados

Não existe tabela `movie` nem `tv_show`.

```
media_file  →  o arquivo físico: path, codecs, duração, tamanho
work        →  a obra: um filme, um episódio, um especial de stand-up
collection  →  agrupamento RECURSIVO: série, temporada, franquia, playlist
work_edge   →  obra ↔ obra: sequel_of, alternate_cut_of, watch_order…
tag         →  com namespace: (mood, melancólico), (format, anime)
person + credit
play_event  →  o log cru
```

O que isso destrava sem migration nova:

- um filme é um `work` sem coleção;
- um episódio é um `work` numa `collection(season)` dentro de uma
  `collection(series)`;
- uma franquia é uma `collection` de `collection`s;
- "Star Wars na ordem Machete" é uma `collection(watch_order)` ordenada (ver §8c);
- "corte do diretor" é `alternate_cut_of`, não um arquivo duplicado.

### `play_event`, não `watched: bool`

`play_event` é a fonte da verdade e nunca é sobrescrito: quando, onde parou, se
largou, se reassistiu. `playback_state` é só um cache derivado pra o "continuar
assistindo" ser um SELECT barato.

Isso parece exagero no M0. É a **fundação do M5** — sem histórico cru não existe
curadoria de verdade, e histórico não se recupera depois.

---

## 8b. Identificação — nunca errar em silêncio (M1)

O Jellyfin decide sozinho e, quando erra, não conta. Aqui a regra é outra:

| confiança | o que acontece |
|---|---|
| ≥ 0.85 | `auto` — aplica sozinho |
| 0.55 – 0.85 | `needs_review` — **não escreve nada na obra**, só marca e pergunta |
| < 0.55 | `unmatched` — nem pergunta, mas guarda os candidatos |
| — | `confirmed` — humano decidiu; o matcher automático **nunca** sobrescreve |

Toda tentativa vira linha em `match_candidate`, com `score` **e** `reasons` —
uma lista de frases legíveis ("ano NÃO confere: 2021 vs 1984", "arquivo parece
episódio, mas o resultado é filme"). Isso é o que aparece na fila de revisão, e
é o que permite responder "por que ele achou que era isso?" seis meses depois.

O score pesa título (0.65, via Jaro-Winkler sobre texto normalizado e sem
acento), ano (±0.25, com penalidade forte se diverge mais de um), concordância
de formato (±0.20 — arquivo que parece episódio não casa com filme) e
popularidade (máx. 0.04, só desempate: se popularidade decidisse, todo arquivo
obscuro viraria o blockbuster de nome parecido).

### Por que AniList e não só TMDB

O TMDB trata anime como série comum: a numeração de temporada não bate com a que
os fansubs usam, e os títulos romanizados casam mal. O AniList indexa romaji,
inglês e nativo ao mesmo tempo, não pede chave de API, e ainda devolve uma cor
de destaque da capa. É acionado quando o caminho tem "anime" ou o arquivo tem
`[Grupo]` na frente.

### `anime` é tag, não `kind`

Um episódio de anime é `kind='episode'` com a tag `format:anime`. Criar um
`kind='anime'` teria sido a saída preguiçosa e teria quebrado o modelo: anime
não é um tipo de mídia, é uma origem — e obras têm várias facetas ao mesmo
tempo. É exatamente pra isso que o namespace de tags existe.

---

## 8c. Ordem de exibição é coleção, não aresta (M2)

**Correção de rumo.** O rascunho original dizia que "ordem Machete" seria uma
cadeia de `work_edge(watch_order)`. Está errado, e vale registrar por quê:

- ler a lista inteira exigiria CTE recursiva a cada consulta;
- inserir um filme no meio obrigaria a reescrever as arestas vizinhas;
- não existe lugar natural pra guardar nome e descrição da ordem.

Ordenação linear é `collection(kind='watch_order')` + `collection_item.position`.
Um `UPDATE` por item numa transação reordena tudo.

As arestas ficam com o que elas fazem bem: **relação semântica de par**, sem
ordem global — `alternate_cut_of`, `remake_of`, `sequel_of`. E toda aresta é
lida dos dois lados: a mesma linha é "é corte alternativo de" pra quem aponta
(`out`) e "tem corte alternativo" pra quem é apontado (`in`). Não existem duas
linhas pra dizer a mesma coisa.

### Tags: namespace obrigatório

`(namespace, value)` em vez de string solta. `mood:melancólico` e
`genre:drama` convivem sem colidir, e a UI agrupa sem heurística.
`tag_namespace` dá rótulo e cor a alguns namespaces — mas **não é lista de
permitidos**: qualquer namespace novo funciona na hora, só cai no grupo "Outros".

Os gêneros do TMDB e do AniList entram como `genre:*` no momento do match. Sem
isso a taxonomia nasceria vazia e o filtro por tag seria um enfeite.

### Filtro composto

`GET /api/works` aceita tags (com `all`/`any`), faixa de ano, faixa de duração,
coleção, estado de identificação e ordenação — tudo numa query só. A coleção
filtra a **subárvore inteira** via `WITH RECURSIVE`: pedir a franquia traz os
episódios das temporadas das séries dentro dela.

O único trecho de SQL montado por concatenação é o `ORDER BY`, e ele vem de uma
whitelist (`order_by()`); todo o resto é bind parameter.

---

## 8d. Preview de seek e sync (M3)

### Folha de sprites, não N miniaturas

Uma imagem por arquivo, com ~120 quadros ladrilhados numa grade 10×N. O player
acha a célula por aritmética:

```
índice = floor(tempo / interval_seconds)
coluna = índice % columns
linha  = índice / columns
```

e recorta com `background-position`. Arrastar a timeline dispara **zero**
requisições — a alternativa (uma miniatura por arquivo) faria dezenas de
requests por segundo durante o arrasto.

O intervalo se adapta à duração (`duração / 120`, com piso de 2s), então um
curta e um filme de 3h geram folhas do mesmo tamanho. A altura da miniatura sai
do aspecto real do vídeo, não de um valor fixo — senão 4:3 fica esticado.

**Custo assumido:** o ffmpeg decodifica o arquivo inteiro pra amostrar. Num
filme de 2h isso leva minutos. Por isso roda em background, uma vez por arquivo,
e o resultado fica em cache pra sempre.

---

## 8e. Os clientes (M4)

A aposta do §9 sobreviveu ao contato com a realidade: `shared` (modelos, Ktor,
repositório) compila para Android **e** iOS, e o que precisou divergir foi
exatamente o previsto — player, navegação de TV, preferências.

Uma divergência não estava prevista e vale registrar: **a URL padrão do
servidor**. No emulador Android, `localhost` é o próprio emulador; o host é
`10.0.2.2`. No simulador do iOS, `localhost` já resolve pro Mac. Errar isso faz
o app "não conectar" sem nenhuma pista, então virou `expect fun defaultBaseUrl()`.

### A TV não reaproveita as telas do celular

De propósito. 10-foot UI é outro paradigma: foco explícito (não há cursor),
tipografia maior, margens de overscan de 48dp, e o controle é a tecla — no
player da TV o `PlayerView` do Media3 fica com `useController = false`, porque
os botões dele são feitos pra toque e viram labirinto de foco no D-pad.

O que se compartilha é tudo abaixo da UI. Tentar compartilhar a UI também é o
erro que faz app de TV parecer celular esticado.

### O `.xcodeproj` é uma casca de 3 arquivos

O Xcode compila `iOSApp.swift` e `ContentView.swift` — 30 linhas de Swift que
hospedam um `UIViewController`. Todo o resto é o framework Kotlin/Compose,
linkado **estaticamente** (`isStatic = true`), o que é por que ele não aparece
em `otool -L`: está dentro do binário. O `.app` final tem 64 MB e ~44 mil
funções Kotlin.

A build phase que chama o Gradle roda **antes** de "Compile Sources": o Swift
precisa do framework já produzido pra enxergar `MainViewControllerKt`.

Duas armadilhas práticas resolvidas ali dentro:

- **O Xcode não herda o ambiente do shell de login**, então `JAVA_HOME` chega
  vazio. O script detecta o JBR do Android Studio sozinho.
- **`NSAllowsArbitraryLoads`** no Info.plist. O servidor é HTTP na tailnet; sem
  isso o iOS bloqueia tudo em silêncio e o app parece "não conectar".

### JDK: o do sistema não serve

O AGP 8.7 não aceita o JDK 24 do sistema. O build usa o JBR 21 que vem com o
Android Studio — que é, aliás, o mesmo com que os outros projetos Android da
máquina compilam.

### Controles próprios são pré-requisito, não capricho

`<video controls>` não permite pendurar nada na timeline. Trocar pelos controles
próprios foi o que destravou o preview — e de quebra deu atalhos de teclado,
auto-hide e a timeline tingida pela cor da obra.

### SSE e a supressão de eco

Cada aparelho tem um `device_id` em `localStorage`. O evento de progresso
carrega esse id, e o emissor descarta o próprio eco — sem isso o player
receberia de volta a posição que ele mesmo acabou de reportar e brigaria com a
própria atualização a cada heartbeat.

O seek remoto só acontece se a diferença passar de 5s, pra dois aparelhos
assistindo juntos não ficarem se corrigindo por décimos.

---

## 8f. Curadoria (M5)

O `play_event` guardado cru desde o M0 é o que torna este milestone possível.
Histórico não se recupera depois — era esse o ponto.

### Dois sinais, deliberadamente separados

**Comportamento** (`play_event`) responde "de que você gosta". **Conteúdo**
(`embedding`) responde "sobre o que a obra é". Comportamento sozinho só
recomenda o que você já viu; conteúdo sozinho é um buscador. Os dois juntos é
curadoria.

### Terminar > assistir

| sinal | peso |
|---|---|
| terminou (ou passou de 92%) | +1.0 |
| passou de 60% | +0.6 |
| deu play e parou antes de 15% | **−0.8** |
| reassistiu | +0.2 a +0.4 |

Dar play não diz quase nada — todo mundo abre e desiste. **Largar aos oito
minutos diz muito.** É o único sinal negativo que se obtém de graça, e a maioria
dos sistemas o joga fora.

Tudo decai por recência com meia-vida de 60 dias: o que você amava há seis meses
pesa metade.

### Embedding local, e por quê

TF-IDF projetado em 256 dimensões pelo *hashing trick* (com sinal, pra colisões
se cancelarem em vez de somarem). É **lexical, não semântico** — "espaço" e
"cosmos" não se aproximam.

Um modelo de embedding de verdade resolveria isso, e o encaixe é trocar uma
função (`embed_document`); o resto do M5 não sabe de onde o vetor veio. Ficou
local de propósito: um servidor de mídia caseiro não deveria depender de API
paga nem mandar sua biblioteca inteira pra um terceiro só pra sugerir filme.

O FNV-1a é implementado à mão porque o `DefaultHasher` da std não garante
estabilidade entre versões do Rust — e um embedding que muda de valor quando o
compilador atualiza é um embedding inútil.

### Tempo é filtro duro, não peso

"Tenho 40 minutos" com um filme de 3h no fim da lista é ruído. Acima de 1.5× o
tempo disponível a obra **some** em vez de aparecer mal colocada.

### O perfil é inspecionável

`GET /api/curation/taste` e um painel na própria tela mostram as afinidades, a
faixa de duração que você termina e a que horas assiste. Mesma regra do M1:
recomendação que não se deixa auditar é adivinhação.

### pgvector: a imagem do Postgres mudou

`postgres:18-alpine` não traz a extensão. O compose passou a usar
`pgvector/pgvector:pg18` — mesma base do Postgres 18 oficial, mesma convenção de
volume, dados preservados na troca.

---

## 8g. Playback pesado (M6)

Adiado desde o M0, e a aposta se pagou: os cinco milestones anteriores existem
porque este não bloqueou nenhum deles.

### Hardware: `-encoders` mente

`ffmpeg -encoders` lista o que foi **compilado**, não o que **funciona**. Neste
container o `h264_nvenc` aparece na lista e morre com "Cannot load libcuda.so.1"
na hora do play. É exatamente o bug que faz o Jellyfin oferecer aceleração que
quebra no meio do filme.

A detecção aqui **codifica cinco quadros sintéticos** com cada candidato, no
boot. O que não codificar não entra. E cada recusa guarda o motivo, exposto em
`GET /api/transcode/capabilities`.

### Capacidade do cliente: perguntar, não presumir

O navegador responde `canPlayType` sobre si mesmo. Lista fixa erraria nos dois
sentidos: o Safari toca HEVC e receberia transcode à toa; um navegador velho
receberia arquivo que não abre.

O ganho apareceu no teste: um arquivo HEVC+AC3 no Chromium virou
`vídeo=copy, áudio=encode` — só o áudio recodifica, porque o navegador **toca**
HEVC. Uma lista fixa teria recodificado o vídeo à toa.

**Pegadinha achada na prática:** o Chromium responde `"maybe"` para
`canPlayType('application/vnd.apple.mpegurl')` e não toca HLS nativo. Testar o
nativo antes do hls.js faz o player carregar a playlist como se fosse mídia e
travar em silêncio. hls.js primeiro; nativo só onde ele não existe.

### Sessões, e por que o seek cria uma nova

O ffmpeg produz do início ao fim, em ordem. Pular pra frente do que já foi
produzido significa recomeçar com outro offset — ou seja, **outra sessão**. Por
isso `start_seconds` é parte da identidade da sessão, não parâmetro dela.

O `-ss` vai **antes** do `-i`: seek por keyframe, instantâneo. Depois do `-i`
seria exato, mas decodificaria tudo até lá.

O reaper mata sessão sem pedido de segmento há 90s. Sem ele, cada seek deixaria
um ffmpeg vivo comendo CPU e uma pasta crescendo até o disco acabar — e o
cliente some sem avisar (fechar a aba não roda cleanup).

### Legendas: três destinos

| tipo | destino | custo |
|---|---|---|
| `subrip`, `mov_text` | extrai pra WebVTT, faixa nativa | nenhum |
| `ass`, `ssa` | WebVTT (perde estilo) **ou** queima | nenhum / transcode |
| `pgs`, `dvdsub` | só queimando — é bitmap | transcode |

ASS carrega posição, fonte, cor e karaokê. Em WebVTT sobra o texto puro. Por
isso a API marca `styled: true` e a interface oferece "queimar" — típico de
anime com letreiro traduzido, onde perder o estilo é perder informação.

---

## 8i. Bibliotecas pela interface

O `MEDIA_PATH` único no `.env` obrigava editar arquivo e reiniciar container pra
mudar o que é varrido. O schema já suportava várias bibliotecas desde o 0001
(`default_kind`) e ganhou `provider_hint` no 0002 — faltava só como escolher.

**A restrição que molda tudo:** o container só enxerga o que está montado nele.
Escolher caminho na interface não ajuda se o Docker não alcança o disco. Por
isso o navegador parte das raízes montadas (`ODEON_MEDIA_ROOTS`) e a criação
valida contra elas — `canonicalize` antes de comparar, senão um symlink dentro
de `/media` viraria porta pro resto do filesystem.

**Três coisas que só apareceram testando:**

- **Bibliotecas aninhadas nascem vazias.** `media_file.path` é UNIQUE, então um
  arquivo pertence a uma biblioteca só. Criar `/media/fillers` com `/media` já
  existente dava "28 vistos, 0 adicionados" — parecia scan quebrado. Agora é
  recusado nos dois sentidos, com mensagem que diz o que fazer.
- **Apagar biblioteca orfanava obras.** O cascade leva os `media_file`, mas
  `work` não tem FK pra library — ficavam cartões que não tocam. O delete agora
  limpa numa transação.
- **O seed automático atrapalhava o caso principal.** Semear `/media` como
  biblioteca de filmes é ótimo quando há vídeos soltos ali, e péssimo quando a
  raiz só tem `Filmes/`, `Séries/`, `Anime/` — reivindicaria tudo como um tipo
  só e depois seria preciso apagar (perdendo o scan) pra separar. O seed agora
  só acontece se houver vídeo solto na raiz.

---

## 9. Clientes: 4 alvos, 2 codebases

| alvo | como |
|---|---|
| Web / desktop | React + TS |
| Android TV | Compose Multiplatform |
| Celular Android | Compose Multiplatform |
| iOS / iPad | Compose Multiplatform |

Quatro apps nativos matariam o projeto na manutenção. O que **não** dá pra
compartilhar — e portanto fica atrás de `expect/actual`:

- **player**: Media3/ExoPlayer no Android, AVPlayer ou VLCKit no iOS;
- **navegação de TV**: foco por D-pad é um paradigma próprio, não é telefone
  esticado.

O resto (rede, modelos, estado, cache offline) é Kotlin comum.

---

## 8h. Elenco e equipe

As tabelas `person` e `credit` existem desde o 0001 — o modelo já as previa. O
que faltava pra serem úteis:

### `provider_key`, ou "Villeneuve" vira 12 pessoas

Sem chave estável do provider, cada filme criaria uma linha nova com o mesmo
nome, e "tudo do Villeneuve" devolveria um filme. `tmdb:person:1234` deduplica
na inserção, via `ON CONFLICT (provider_key)`.

### Cortar é parte do trabalho

Um filme grande tem 200 nomes na equipe. Importar tudo enterraria o diretor no
meio dos assistentes de efeitos. O TMDB entra por **allowlist de cargo**
(`Director`, `Screenplay`, `Composer`…) e o elenco é cortado nos 15 primeiros —
que é a ordem de relevância do próprio TMDB.

O AniList é mais verboso ainda ("Key Animation", "2nd Key Animation"), então lá
a filtragem é por prefixo em vez de igualdade.

`credit.role` continua sendo **texto livre**, sem CHECK: provider inventa cargo
o tempo todo. O que a interface destaca é decidido pela tabela `credit_role`,
não pelo schema.

### Dublador é informação de primeira classe

Em anime, muita gente escolhe o que assistir pelo elenco de voz. O AniList
entrega personagem + dublador, e isso vira `role='voice'` com `character_name`.
O TMDB não tem equivalente pra anime.

### A afinidade por pessoa exige 2+ obras

É a peça que faz esta etapa render no M5. Mas com uma obra só, o elenco inteiro
de um filme que você gostou viraria "gosto favorito" — 40 pessoas com afinidade
+1.0 a partir de uma noite. Duas obras já são evidência fraca mas real.

Só papéis de destaque entram: o compositor de um filme que você largou não diz
nada sobre você.

---

## 9b. Autenticação

### Sessão opaca, não JWT

JWT é stateless, o que soa bom até você querer deslogar um aparelho perdido.
Num servidor pessoal não há escala que justifique abrir mão de revogação: a
linha some da tabela e acabou.

O token tem 256 bits de `OsRng` e **não é guardado** — guarda-se o SHA-256 dele.
Vazar o banco não dá sessão a ninguém. Argon2 aqui seria desperdício: a ameaça
contra token de alta entropia não é força bruta, é vazamento.

Senha é outra história: **Argon2id**, resistente a GPU.

### O problema que domina o desenho: mídia não manda header

`<video src>`, `<img src>`, `<track src>` e `EventSource` não mandam
`Authorization`. E cookie cross-origin exige `SameSite=None; Secure` — ou seja
HTTPS, que não existe num servidor HTTP na tailnet.

Três caminhos, com escopos diferentes:

| caminho | onde vale |
|---|---|
| `Authorization: Bearer` | toda a API; é o que os clientes Kotlin usam |
| cookie `odeon_session` | quando web e API forem a mesma origem |
| `?token=` na query | **só nas rotas de mídia** |

O terceiro é um compromisso consciente: token em query vaza pra log de acesso e
histórico do navegador. Restringi-lo à mídia limita o estrago. Se um dia isto
for exposto de verdade, o certo é emitir um token de mídia curto e separado.

### Mensagem única no login

Usuário inexistente, usuário sem senha e senha errada devolvem exatamente a
mesma resposta. Distinguir entregaria de graça a lista de usuários válidos.

### O setup se fecha sozinho

Enquanto `password_hash IS NULL` em todo mundo, `/api/auth/setup` responde.
Depois disso, 403 permanente. E o setup **reivindica** o usuário semeado no M0
em vez de criar outro — assim o histórico de reprodução acumulado continua sendo
da mesma pessoa, e não fica órfão numa conta fantasma.

### Colateral: o `state.user_id` morreu

Até aqui havia um usuário resolvido no boot, guardado no `AppState`. Ele sumiu:
todo handler que precisa saber quem é agora recebe `AuthUser` por extractor. Foi
a mudança mais espalhada desta etapa, e é o que torna o multiusuário real em vez
de decorativo.

---

## 10. Segurança — estado atual

- **Autenticação**: feita (ver §9b) — Argon2id, sessões revogáveis, papéis.
- **CORS apertado** (ver §10b).
- **HTTPS**: opcional, desligado por padrão (ver §10c).
- **Token de mídia na query**: ver a ressalva em §9b.

---

## 10c. HTTPS

**Por que existe, já que a Tailscale criptografa.** Não é confidencialidade — é
**contexto seguro**. Service Worker, PWA offline, `crypto.subtle` e parte do
Media Session API simplesmente não existem em HTTP. Sem TLS, essas portas ficam
fechadas pra sempre.

**Certificado: Tailscale, não auto-assinado.** `tailscale cert` emite um Let's
Encrypt real pra `*.ts.net`. Auto-assinado obrigaria instalar CA em cada
aparelho — e em Android TV isso é um suplício. O script `certs/dev-cert.sh`
existe só pra testar a camada, não pra uso real.

**TLS no processo, não num proxy.** Caddy ou nginx resolveriam, mas seria mais
um container pra um servidor de uma pessoa. O axum faz isso, e a renovação
continua sendo `tailscale cert` num cron.

**`tls-openssl`, não `tls-rustls`:** o provider padrão do rustls 0.23 é o
aws-lc-rs, que exige cmake na imagem. O openssl já estava lá por causa do
reqwest.

**Os clientes não têm URL fixa.** Ligar HTTPS não exige editar nada:

- **apps**: o usuário digita o host e o app sonda `https://host:8443` antes de
  `http://host:8080`, ficando com o primeiro que responder `/api/health`. Se
  escrever o esquema, a escolha é respeitada e o outro não é tentado — tentar
  https por baixo de um `http://` explícito seria surpresa.
- **web**: a API é deduzida de `window.location` — mesmo host, mesmo esquema,
  porta conforme o esquema.

**Mixed content é a armadilha real.** Uma página HTTPS não pode chamar uma API
HTTP: o navegador bloqueia, e isso inclui `<video src>`. Sem tratamento, isso
parece "servidor fora do ar". A web detecta a combinação e diz o que fazer.

**Detalhes que parecem pequenos e não são:**

- **HSTS só na resposta HTTPS.** Mandá-lo em HTTP prenderia o navegador num
  HTTPS que talvez não exista, e o usuário ficaria sem acesso.
- **Redirect 308, não 302.** O 302 transforma POST em GET — um login
  redirecionado perderia o corpo no caminho.
- **Cert sem key (ou vice-versa) derruba o boot.** Subir em HTTP achando que
  está protegido é pior que não subir.
- A porta HTTP **continua de pé** mesmo em modo estrito, só redirecionando.
  Fechá-la deixaria quem digitou `http://` sem resposta nenhuma.

---

## 10b. CORS: a regra do mesmo host

Lista fixa de origens era a saída óbvia e a errada. O servidor é alcançado por
nomes que ele não conhece de antemão — `rog`, `odeon.tailnet.ts.net`, um IP — e
uma allowlist quebraria o acesso em silêncio no dia em que o nome mudasse.

A regra é comparativa em vez de declarativa:

1. origem na `ODEON_ALLOWED_ORIGINS` → aceita;
2. origem em loopback (`localhost`, `127.0.0.1`, `::1`) → aceita, é o dev;
3. **host da origem == host pelo qual a requisição chegou**, ignorando a porta
   → aceita.

A regra 3 é a que carrega o peso: o front em `http://rog:5174` falando com a API
em `http://rog:8080` passa sem configurar nada, e `http://evil.com` não passa.
A comparação é do host inteiro — `rog.evil.com` **não** casa com `rog`.

Com origem específica dá pra ligar `allow_credentials`, o que antes era
impossível: o CORS proíbe `Access-Control-Allow-Origin: *` junto de credenciais.
Isso é o que faz o cookie de sessão finalmente valer alguma coisa.

`Content-Range` e `Accept-Ranges` entram em `expose_headers` porque não estão na
lista segura do CORS — sem isso, um player baseado em `fetch` não enxerga o
tamanho do vídeo.

Existe a escotilha `ODEON_ALLOWED_ORIGINS=*`, que volta ao comportamento antigo
e **avisa no boot**. Ela existe pra desbloquear alguém às 2h da manhã, não pra
uso normal.

---

## 11. Riscos conhecidos

**Async Rust.** O borrow checker em código de webserver é tranquilo; a dor está
em `Pin`, lifetimes em `async` e estado compartilhado. Mitigação: axum + sqlx é
caminho batido, e FFmpeg como subprocesso evita FFI.

**O M4 é onde projetos assim morrem.** Chegar lá com backend estável e web
funcionando é o que torna o resto viável.

**O parser de nome de arquivo é um poço sem fundo.** Ele nunca fica "pronto" —
por isso a fila de revisão manual do M1 é parte do design, não um plano B.
