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

## 8j. A pasta como unidade de identificação

**Correção de rumo, e a mais cara de todas.** Numa biblioteca real de 17.503
arquivos, 7.568 ficaram por identificar. Decidir arquivo por arquivo era
inviável — e desnecessário:

| | |
|---|---|
| arquivos por identificar | 7.568 |
| **diretórios distintos** | **578** |
| diretórios já casados que apontam para UMA série | 474 de 487 (97,3%) |

O nome da série está na pasta, limpo, mesmo quando o nome do arquivo é
ilegível. Uma decisão humana em `/media2/TV Show/Naruto Shippuden` resolve 499
arquivos, e resolve os que chegarem depois.

`identification_scope` (0008) guarda essa decisão: biblioteca, pasta,
recursividade, provider e o modo de numeração. O `run_matching` a consulta antes
de buscar — **pasta decidida não vira pergunta de novo**. Sem isso o escopo
seria registro histórico e o backlog voltaria a cada scan.

Quando mais de um escopo casa, vence o mais **específico**: a decisão sobre
`Serie/Temporada 2` é mais informada que a sobre `Serie`.

### `dry_run` é passo obrigatório, não conveniência

Aplicar um escopo escreve centenas de linhas. A interface não oferece o botão
antes de mostrar o preview com o `SxxExx` resolvido e o título que cada arquivo
vai receber. É o §8b aplicado à escala: a diferença entre "não escrever quando
está em dúvida" e "não escrever sem mostrar" é só o tamanho da operação.

### `ignored`: o estado que faltava

16,3% da fila (1.234 arquivos) estava em `Featurettes/`, `Extras/`,
`Deleted Scenes/`. Material que **nenhum provider cataloga** — não era trabalho
pendente, era trabalho impossível ocupando a fila.

`ignored` não é "não identificado", é "não se aplica". Adicioná-lo foi um
`ALTER TABLE`, que é literalmente o argumento do §5 para ter escolhido
`text` + `CHECK` em vez de ENUM.

### Propagação: pelo diretório, nunca pelo título

`apply_to = directory | series` acha os irmãos pela pasta. Título adivinhado
colapsaria "Naruto" e "Naruto Shippuden", que são pastas vizinhas. E `confirmed`
nunca é sobrescrito por propagação: quem decidiu foi um humano, e um vizinho não
desfaz isso.

Um irmão **sem número de episódio próprio não vira `confirmed`** — receberia a
série certa e um episódio inventado. Fica na fila, agora sabendo qual é a série.

### `parse_override`: a correção humana que se perdia

A busca manual deixava digitar o título certo, e o descartava: ela mutava um
`Guess` local só pra montar a consulta, e o `confirm` re-derivava tudo do
caminho. Escolher a série certa para `Frieren - 37.mkv` ainda buscava temporada
1, episódio 37.

Guardado em `work.parse_override` (0009), o override sobrevive ao confirm, ao
re-scan e ao re-match — é decisão humana, e o `reset` a preserva.

### O `reset` precisava ser simétrico

Ele limpava estado e artwork mas deixava título, sinopse, `external_ids`,
coleções, créditos e tags do match desfeito. O resultado era uma obra "não
identificada" ainda exibindo o nome da série errada.

Agora remove tudo que veio do provider e preserva o que é humano: tag manual,
playlist, ordem de exibição, e o `parse_override`.

Isso expôs um bug anterior: `ensure_collection` inseria sem preencher `origin`,
que tem default `'manual'` — as 494 séries e temporadas criadas pelo matcher se
apresentavam como feitas à mão, e sem a distinção o reset apagaria a coleção
errada, ou nenhuma.

---

## 8k. Score: adicionar evidência, não afrouxar o limiar

O teto de um episódio **sem ano** é exatamente `0.65 + 0.05 + 0.08 + 0.04 =
0.820`, e `AUTO_THRESHOLD` é 0.85. Medido: **22.219 de 33.114 candidatos (67%)**
eram estruturalmente incapazes de entrar sozinhos, e 653 deles tinham título
IDÊNTICO ao do provider. Casamento perfeito que o score não conseguia expressar.

Baixar o limiar seria a saída errada: o número deixaria de significar "tenho
certeza" e passaria a auto-aplicar todo candidato de 0.78, inclusive os errados.
O Jaro-Winkler é generoso com prefixo comum — "Naruto" contra "Naruto
Shippuden" pontua alto.

A saída foi **adicionar a evidência que faltava**. Uma pasta de série tem uma
série só; se cinco arquivos da mesma pasta escolheram a mesma obra, o sexto
escolhendo ela não é coincidência.

**Três freios, e eles são o ponto:**

- `similarity >= 0.90` — a corroboração CONFIRMA um título que já estava bom,
  nunca resgata um ruim. Sem isso, uma pasta inteira errada se auto-referendaria
  com alta confiança;
- teto de 0.10 — sozinha nunca leva nada de 0.55 a 0.85;
- mínimo de 3 vizinhos — dois concordando é acaso comum.

**Dois riscos registrados.** A evidência é **correlacionada**: pasta errada erra
junto e com convicção — por isso o motivo entra na lista de `reasons`. E o score
fica **dependente da ordem de processamento**, porque numa primeira execução os
vizinhos ainda não têm candidato; só rende de fato numa segunda passada. Isso é
aceitável porque o §8b pede que o score seja *explicável*, não idêntico entre
execuções.

### Parser: as classes que só um acervo real mostra

Sete formas de nomenclatura que não eram reconhecidas, todas medidas antes de
serem corrigidas: `T##E##` (luso-brasileiro), `S07 E 20`, "3ª Temporada
Episódio 04", `EP13`, `Episódio 12` por extenso, índice na frente
(`094. Tom & Jerry`), e endereço de site carimbado pelo tracker
(`Pica-Pau.WEB.DUB-WWW.BLUDV.COM` — 489 arquivos).

**As regras arriscadas são condicionadas ao `library.default_kind`.** Índice na
frente destruiria `007 - Cassino Royale`; `EP` num acervo de música é disco, não
numeração. Elas só ligam em biblioteca de episódios — o contexto que o schema já
tinha desde o 0001.

O endereço de site sai **antes** da normalização, junto com o separador que o
segue. Removê-lo sozinho deixaria ` - Os.Jetsons`, que TEM espaço — e aí a
heurística de scene release não converte os pontos, e o título vira
"Os.Jetsons".

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

### O `?token=` não se propaga pros segmentos HLS

O ffmpeg escreve a playlist com nomes RELATIVOS (`seg00000.ts`), e resolução
relativa **descarta a query string**. O player pedia a playlist com `?token=`,
resolvia os segmentos sem ele, recebia 401 — e o hls.js reportava
`fragLoadError`, sem mencionar autenticação. Parecia erro de rede.

O player manda `Authorization: Bearer` via `xhrSetup`, que vale pra todo pedido
do hls.js. Header e não query: o `?token=` existe porque `<video src>` não manda
header — mas aqui quem busca é XHR, que manda. Assim o token não vaza pra log de
acesso nem histórico.

Pela mesma razão, `spriteInfo` no cliente buscava `/api/media/{id}/scrub` sem
credencial nenhuma e tratava o 401 como "não há sprite". Os sprites já gerados
nunca chegaram a aparecer para ninguém.

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

**`from_pem_chain_file`, não `from_pem_file`.** O primeiro chama
`set_certificate_chain_file` e manda a cadeia inteira; o segundo chama
`set_certificate_file` e manda **só a folha**. O arquivo do `tailscale cert` tem
4 certificados (folha → YE2 → Root YE → ISRG Root X2), e mandando só a folha o
cliente não acha o emissor: `unable to verify the first certificate`. Navegador
de desktop às vezes salva pescando o intermediário por AIA — Android TV não, que
é justamente o aparelho pelo qual este parágrafo existe.

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

## 12. Operações longas: estado no banco, não no processo

Varredura, identificação, sprites e embeddings viviam em `Arc<Mutex<Status>>`.
O que isso custou numa implantação real:

- um `systemctl stop docker` matou uma varredura de 17 mil arquivos no meio, e
  depois do restart o status dizia `running: false` — **indistinguível de "nunca
  rodou"**;
- uma reaplicação de escopo de 16 minutos morreu porque o `cargo watch`
  reiniciou o processo, e não havia registro de que 59 de 500 tinham sido feitas;
- não havia como cancelar: parar exigia matar o processo, perdendo tudo.

A tabela `job` (0011) resolve os três. Três decisões merecem registro:

**O job ENVOLVE, não substitui.** As structs de status continuam iguais e são
serializadas inteiras em `progress` — os endpoints `/status` devolvem exatamente
o mesmo JSON. Há quatro alvos de cliente lendo aquilo, e quebrá-los por causa
disto não se justifica.

**`interrupted` é um estado próprio**, distinto de `failed` (a operação deu
erro) e de `cancelled` (alguém pediu). A recuperação roda **antes de servir**:
o índice único de job ativo por tipo travaria o servidor para sempre se o job
morto continuasse "rodando" — toda varredura nova seria recusada alegando
varredura em andamento.

**O cancelamento é cooperativo.** Quem pede só marca a coluna; o worker para no
próximo ponto seguro. Interromper entre gravar a obra e gravar a coleção dela
deixaria estado pela metade. A granularidade varia com o custo do item: a cada
50 arquivos no scan, a cada UM sprite — porque um sprite leva minutos, e esperar
50 seria esperar uma hora.

**O encadeamento (`?then=match`) só dispara se a varredura CONCLUIU.** Depois de
um cancelamento, encadear seria identificar sobre um acervo pela metade. A
decisão vem do estado do job, não da existência de `finished_at` — que é
carimbado nos dois casos.

---

## 13. Três medições que contrariaram a intuição

### Sprites: o custo era proporcional à duração, não ao acervo

O filtro `fps=1/N` obriga o decode do arquivo inteiro pra amostrar 120 quadros.
Com 7.879 horas de conteúdo e 1.019 arquivos acima de 1h, gerar tudo levaria
**~797 horas**.

`-skip_frame nokey` — uma flag — leva para **~14 horas**. Medido num 1080p de
20 minutos: 126,0s → 17,5s.

**A alternativa mais elaborada perdeu.** Blocos de 12 entradas com `-ss` antes
do `-i`, que a análise prévia estimava em 22×, deram **28,1s** — pior que a
flag, e muito mais complexos. Fica registrado pra ninguém refazer a medição
achando que vai ganhar.

A geometria não muda (verificado: 1600×1080 nos dois), o que importa porque o
player acha a célula por aritmética (§8d). O quadro exibido é o quadro-chave
mais próximo, deslocado em no máximo um GOP — o mesmo compromisso que o §8g já
aceita no seek do transcode.

### O índice HNSW existia e era inalcançável

`ORDER BY 1 - (embedding <=> $2) DESC, updated_at DESC` com um `CASE` em volta.
Três coisas ali impedem o pgvector ao mesmo tempo: o `CASE`, a inversão, e a
segunda chave de ordenação.

```
antes:  Seq Scan on work (17.498 linhas) + Sort
agora:  Index Scan using work_embedding_idx
```

A causa raiz era o truque `NULLIF($2,'')::vector`, que resolvia "usuário novo" e
"usuário com histórico" na mesma query — **um caminho pagava pelo outro**. São
duas queries agora. Ganho duplo: o índice entra, e os quatro `JOIN LATERAL`
deixam de rodar para 17 mil linhas antes de ordenar.

### `taste::build` escalava com a biblioteca errada

Trazia `work_tag ⋈ tag` inteiro (13.728 linhas) e todos os créditos featured
(61.919) para casar, em Rust, com o punhado de obras que a pessoa assistiu — a
cada `/for-you` e cada `/taste`. Filtrando por `ANY($1)`:

| | antes | depois |
|---|---|---|
| `/curation/taste` | 0,95s | 0,012s |
| `/curation/for-you` | 0,96s | 0,035s |

---
## 11. Riscos conhecidos

**Async Rust.** O borrow checker em código de webserver é tranquilo; a dor está
em `Pin`, lifetimes em `async` e estado compartilhado. Mitigação: axum + sqlx é
caminho batido, e FFmpeg como subprocesso evita FFI.

**O M4 é onde projetos assim morrem.** Chegar lá com backend estável e web
funcionando é o que torna o resto viável.

**O parser de nome de arquivo é um poço sem fundo.** Ele nunca fica "pronto" —
por isso a fila de revisão manual do M1 é parte do design, não um plano B.

**A corroboração de vizinhos é correlacionada (§8k).** Se uma pasta inteira for
identificada errado, os arquivos dela se referendam mutuamente com alta
confiança. Os três freios reduzem a chance, não a eliminam. O motivo gravado é o
que permite descobrir depois.

**O score depende da ordem de processamento (§8k).** Numa primeira execução os
vizinhos ainda não têm candidato. O mesmo arquivo pode pontuar diferente entre
duas execuções — aceitável porque o §8b pede score *explicável*, não
determinístico, mas é uma propriedade a lembrar antes de comparar números.

**A heurística de pasta de extras vai errar em algum caso (§8j).** Um episódio
de verdade numa pasta chamada `Bonus/` seria marcado `ignored`. É reversível e a
razão fica gravada, mas ninguém revisa o que saiu da fila.

**`sqlx::migrate!` embute as migrações em tempo de compilação.** Adicionar um
`.sql` não basta — se nada que o compilador rastreia mudar, o binário antigo
sobe dizendo "migrations em dia" e não aplica nada. Custou uma hora de confusão.


---

## 12. A era do redesenho — Fase 1: o painel

Seis milestones de substância e uma interface que era a casca funcional que
cresceu junto com eles. A tela de entrada (`para você`) é onde a tese do projeto
deveria ficar óbvia — e era justamente onde ela sumia.

### O que estava errado, concretamente

1. **A ação mais gritante da tela era manutenção.** A topbar tinha onze pílulas
   quase idênticas misturando navegação, identidade e administração, e a única
   em amarelo sólido era `identificar`. Quem abria o Odeon pra assistir
   encontrava um painel de operação.
2. **Não havia hierarquia.** O #1 (score 56) tinha o mesmo peso visual do #24
   (score 41). O M5 produz um ranking e a tela jogava esse ranking fora.
3. **Os motivos eram o elemento mais fraco.** "Todo item diz por quê" é pilar do
   projeto, e na tela isso era um chip cinza de 10px.

### Marquise

O Odeon é uma sala de cinema — o nome já diz. Serifa de display no que é obra,
sans no que é interface; um herói "esta noite"; e o amarelo como luz de
marquise, incluindo a fileira de lâmpadas na borda superior do herói.

### A regra de cor: amarelo é sistema, cor da obra é arte

O M3 fez a cor dominante do pôster tingir player, cartões e timeline. No painel
isso tinha ido longe demais: ela tingia o rótulo da série e o número do score, e
com uma obra por linha a tela virava um mostruário de cores onde o preto e
amarelo não se sustentava.

A divisão agora é explícita:

| | onde |
|---|---|
| **amarelo** (`--accent`) | ação, foco, aba ativa, score — o *sistema* |
| **cor da obra** (`--accent-work`) | halo do herói e borda do pôster — a *arte* |

O amarelo sólido ficou reservado a **um** elemento por tela: `▸ Assistir`.

### A manutenção saiu da barra

`varrer`, `identificar`, `sprites` e `embeddings` foram pra uma gaveta
`Servidor`. Mas o **progresso continua no fluxo principal**: esconder numa
gaveta o aviso de que dezessete mil arquivos estão sendo varridos seria perder
exatamente a informação que a implantação deste servidor ensinou a mostrar
(§ jobs). Gaveta é pra *disparar*, não pra *acompanhar*.

As abas deixaram de ser cápsulas e viraram texto sublinhado. Como cápsulas, elas
eram indistinguíveis dos botões de manutenção que dividiam a barra — a forma
dizia "isto é um botão" quando a verdade é "isto é onde você está".

### Diversidade é apresentação, não pontuação

Quatro dos cinco primeiros eram da mesma série. A tentação é mexer no score, e
seria errado: **o score está certo**. Um perfil concentrado faz o vizinho mais
próximo do vetor de gosto ser sempre a mesma série. Mexer no score pra resolver
layout estragaria o "por quê" de cada item, que é o que a tela promete.

Então o corte é de apresentação (`curation::diversify`): no máximo duas por
série na frente, e o excedente é **empurrado pro fim, nunca descartado** — numa
biblioteca pouco variada pode não haver o que colocar no lugar, e devolver meia
tela vazia é pior que repetir.

### O backdrop já estava no disco

`work.artwork` guardava `poster`, `backdrop` e `still` desde o M1; só o `poster`
chegava ao cliente. O herói precisa de 16:9 — recortar um pôster 2:3 dá
enquadramento ruim em toda obra — então os outros dois passaram a sair na
projeção.

**A pegadinha:** `WorkListItem` é `sqlx::FromRow`, então *toda* query que projeta
nessa struct precisa devolver as colunas novas. São quatro (`curation`, `works`,
`graph`, `people`), e esquecer uma só falha quando aquela rota é chamada.

### A fonte é do sistema, de propósito

`ui-serif`/Georgia, zero bytes. Um servidor que roda numa tailnet não deveria
depender de CDN de fonte pra desenhar a tela, e vendorar um `.woff2` é uma troca
de uma linha se um dia valer a pena.

---

## 13. A era do redesenho — Fase 2: o player

### Sala escura, não modal

O player era um card sobre o painel a 92% de opacidade: dava pra ver o catálogo
piscando atrás da cena. Isso lê como caixa de diálogo, não como sala. Agora ele
é preto de verdade, ocupando a viewport inteira, e o `object-fit` do vídeo é
`contain` — **as barras pretas são honestas.** Um 4:3 numa tela 16:9 tem barras;
fingir o contrário com `cover` recorta cabeça de gente.

O vocabulário da marquise veio junto — serifa de display no título, lâmpadas na
borda do scrim — mas em dose menor que no painel: ali embaixo passa imagem em
movimento, e o que decora atrapalha.

### O player estava mentindo em quatro lugares, pelo mesmo motivo

Medido nesta máquina: uma sessão com `start=600` num arquivo de 1355s entrega um
stream de **755s**, com `currentTime` começando em zero. O `-ss` vai antes do
`-i` (§8g), então o ffmpeg produz *a partir* do offset — e o `<video>` não sabe
que existe um arquivo maior atrás dele.

Confiar no elemento fazia quatro coisas errarem de uma vez:

| sintoma | causa |
|---|---|
| duração total errada (1:33 num episódio de 22:35) | a playlist é `EXT-X-PLAYLIST-TYPE:EVENT` e cresce enquanto o ffmpeg escreve |
| "continuar de onde parou" pulava o dobro | `currentTime = resumeFrom` sobre um stream que **já** começava em `resumeFrom` |
| progresso gravado no lugar errado | mandava `currentTime` cru, deslocado pelo offset |
| preview de seek mostrava o quadro errado | a folha de sprites é indexada por tempo de arquivo |

A correção é uma só: **tudo trabalha em tempo de arquivo.** `offset +
currentTime` para a posição, e o total vem do `ffprobe`, que é quem sabe o
tamanho real. O `<video>` passa a ser um detalhe de transporte.

### A região inalcançável é desenhada, não escondida

Com o total correto, a timeline mostra o arquivo inteiro — inclusive o pedaço
que **esta** sessão não produziu. Esconder seria voltar a mentir; deixar clicável
seria falhar em silêncio. Então a faixa não-produzida é hachurada, e clicar nela
diz o que fazer em vez de não fazer nada.

Alcançar aquele trecho de verdade exige outra sessão — `start_seconds` é parte da
identidade da sessão, não parâmetro dela (§8g). Isso fica pra uma fase própria: é
mais trabalho que todo o resto desta junto.

### O selo do modo virou peça

Direct Play / Remux / Transcode com os motivos é uma das melhores ideias do
projeto — a pergunta que o Jellyfin nunca responde — e estava num chip de 11px no
canto superior. Agora é um selo legível na barra de controles, com `?`, e os
motivos abrem num cartão desenhado em vez de uma `<ul>` crua.

As legendas saíram de dentro desse painel. Escolher faixa de legenda e auditar a
negociação de codec são duas coisas sem relação nenhuma, e estavam no mesmo lugar
só porque foram implementadas no mesmo milestone.

### Controle é sistema, e sistema é amarelo

A timeline, o knob e o botão grande usavam `--accent-work` — o player inteiro
ficava magenta num episódio de pôster magenta. Pela regra da Fase 1 (§12) a cor
da obra é *arte*: sobrou pra ela o halo atrás do palco, e mais nada.

---

## 14. A era do redesenho — Fase 3: a biblioteca

### 14.657 episódios não são uma biblioteca

A tela mostrava um cartão por obra. Com 17.498 obras, das quais 14.657 são
episódios, isso não é uma biblioteca — é `ls` com pôster. As séries **já
existiam no grafo desde o M1** (`collection(series)` → `collection(season)`); o
que faltava era a tela usá-las.

`GET /api/library` agrega: uma entrada por série, uma por obra avulsa. O grupo
sobe episódio → temporada → série, e quando a temporada não tem série mãe ela
mesma vira o grupo — o mesmo `COALESCE` que o `series_title` já fazia, agora
carregando o id. Resultado neste acervo: **7.969 entradas** no lugar de 17.498.

`/api/works` continua existindo e continua plano, porque **dentro** de uma série
o que se quer é a lista de episódios. Duas perguntas diferentes, duas rotas. O
drill-in reaproveita o filtro de coleção, que já resolvia a subárvore inteira
com `WITH RECURSIVE` (§8c).

### O número no cabeçalho mentia

Dizia "Biblioteca 300" porque o front pedia `limit=300` fixo — e não havia como
ver a 301ª. O backend sempre aceitou `limit`/`offset`. Agora a resposta carrega
`count(*) OVER ()`, então "120 de 7.969" sai na mesma ida ao banco.

### A ordenação padrão mostrava o pior primeiro

Ordenar por título põe `001 - Draw My Life As a Gamer` na frente, porque número
vem antes de letra. As 8.616 obras identificadas ficavam depois de quatro mil
arquivos sem match — a primeira tela da biblioteca era o lixo do acervo.

O padrão virou `featured`: `(poster IS NULL), title`. Não esconde nada — o que
não foi identificado continua ali, no fim.

### Ignorada é obra descartada de propósito

`/api/works` nunca filtrou `match_state = 'ignored'`, então as 1.234 obras que
alguém mandou ignorar voltavam como cartão cinza. Agora ficam fora por padrão e
só reaparecem quando o filtro pede por elas — o chip existe justamente pra isso.

### O índice que faltava desde o 0001

Investigando por que o filtro "sem match" parecia não funcionar na tela,
descobri que ele funcionava: a resposta demorava **11 segundos** e o screenshot
capturava o estado velho.

A causa não era volume. `collection_item` tem PK `(collection_id, work_id)`, que
serve pra "quais obras estão nesta coleção" — mas a pergunta que a biblioteca faz
o tempo todo é a inversa, "de que série é este episódio?", e essa não usa o
prefixo do índice. Era varredura sequencial a cada obra:

|  | antes | depois |
|---|---|---|
| o LATERAL da série | 6.557 ms | 119 ms |
| `/api/works` | 6,9 s | 1,10 s |
| `/api/library` | 11,4 s | 1,30 s |
| com filtro | 4,6 s | 0,15 s |

**A tela plana já estava lenta antes desta fase** — 6,9 s pra abrir a
biblioteca. O agrupamento só tornou o problema impossível de ignorar. Uma linha
de migração (0012).

### Dentro da série, a temporada é um nível

O drill-in nasceu achatando: 77 cartões numa grade só, na ordem certa
(`season_number`, depois `episode_number`) mas sem nenhuma quebra. Em Malcolm
seriam 151. A temporada existia no grafo e sumia na tela — que é a mesma falha
que esta fase veio corrigir uma camada acima, com a série.

Agora cada temporada é uma faixa, com a mesma `.strip` do painel: repetir o
vocabulário é o que faz as telas parecerem o mesmo produto.

**O agrupamento é por `work.season_number`, não pela coleção `season`.** Medi
antes de decidir: das 8.410 obras dentro de uma temporada, **nenhuma** está sem
`season_number`, e ele nunca diverge do número no título da coleção — e os 461
títulos de temporada são todos "Temporada N", ou seja, não carregam informação
que o número já não tenha. Agrupar pelo campo dispensa uma coluna nova em
`WorkListItem`, e cada coluna nova ali custa quatro projeções SQL.

Quem não tem `season_number` cai num grupo "Sem temporada" em vez de sumir, e a
temporada 0 vira "Especiais".

### Episódio mostra o episódio, não a série

Com a temporada virando faixa, ficou óbvio o que o pôster fazia ali: uma
temporada de 21 episódios eram 21 cópias da mesma imagem. A arte ocupava a tela
inteira sem distinguir nada.

O `still` — o quadro daquele episódio — está baixado desde o M1 e passou a sair
na API na R1. **7.258 dos 14.657 episódios têm um.** O cartão de episódio agora
prefere o `still`, cai pro pôster da série quando não há, e só então pro
gradiente com o título. Medido em *Naruto*, que tem cobertura desigual:

    Temporada 1:  26 com still
    Temporada 2:  26 caem no pôster
    Temporada 3:  28 caem no pôster
    nenhum cartão chega ao gradiente

**O formato mudou junto:** os stills são 780×439, ou seja 16:9. Enfiar isso numa
moldura de pôster 2:3 cortaria dois terços do quadro, então o episódio ganhou
cartão paisagem e uma coluna mais larga — senão cabem oito por linha e o quadro
fica do tamanho de uma miniatura. O número do episódio virou selo, porque num
still não há nada que diga a ordem.

### Cartão sem arte precisa dizer alguma coisa

5.644 obras não têm pôster nenhum. O cartão delas era um retângulo de gradiente
com um badge. Agora o título ocupa até quatro linhas e o rodapé traz tipo,
episódio e ano — informação que já estava no objeto e não aparecia.

---

## 15. A era do redesenho — Fase 4: coleções

### A aba estava competindo com a biblioteca, e perdendo

Era uma árvore de 576 nós sempre expandida — 115 séries e 461 temporadas — com
um painel de detalhe vazio ao lado. Desde a R3 a biblioteca navega série →
temporada → episódio com arte e progresso; a árvore aqui só repetia isso, pior.

Então a aba virou o que **só** ela faz: ordens de exibição, playlists e
franquias no topo, com cartão de capas empilhadas. As séries e temporadas do
matcher continuam alcançáveis, recolhidas atrás de um botão que diz quantas são.

**O diagnóstico que decidiu isso:** o acervo tinha **zero** playlists e **zero**
ordens de exibição. A "ordem Machete" é caso de uso de primeira classe no §8c e
nunca tinha sido usada uma vez — em parte porque a tela não convidava a criar,
e povoar uma exigia abrir o detalhe de cada obra, uma por uma.

### "SÉRIE · 0" em cima de 70 episódios

`item_count` contava só filhos diretos, e episódio pertence à temporada, não à
série. Toda série aparecia como vazia.

Agora conta a subárvore — e a primeira versão custou caro: um `LATERAL` com
`WITH RECURSIVE` correlacionado em `c.id` é correto e roda **576 vezes**, o que
deu 4,9 s na árvore. A versão que ficou faz a recursão **uma vez**:
`descendencia` casa cada coleção com toda a sua subárvore e o `GROUP BY` agrega
em cima disso.

    árvore inteira:  4,9 s  →  0,77 s

`count(DISTINCT ci.work_id)` porque nada impede a mesma obra de estar numa
temporada e numa ordem de exibição dentro da mesma franquia.

### Criar deixou de ser `prompt()`

Era `prompt("Nome da coleção:")` seguido de `confirm("É uma ordem de exibição?
(Cancelar = playlist comum)")` — duas caixas cinza do navegador no meio da sala
escura, e a segunda impossível de responder sem já saber o vocabulário. Agora é
um formulário embutido onde cada tipo traz o que ele significa.

### Arrastar manda a lista inteira

A reordenação era ↑/↓, uma troca por clique — o próprio código chamava de
"refinamento do M3". Agora é arrastar, e o `PUT /order` recebe **todas** as
posições, não só as duas que mudaram: arrastar do 8º pro 2º desloca seis
vizinhos, e mandar par a par deixaria o meio inconsistente se uma requisição
falhasse.

---

## 16. A era do redesenho — Fase 5: revisão

### A tela que mais funciona é a que menos se mexeu

A revisão é o melhor do Odeon: candidato a candidato, com os motivos do score
escritos e a pasta como unidade de decisão. Redesenhar aqui era o maior risco
da era inteira — estragar um fluxo bom em nome de consistência visual.

Então o corte foi: **vestir e tapar buracos, sem tocar no fluxo de decisão.**

### O buraco era paginação

`/api/review/scopes` sempre aceitou `offset`. A tela pedia 50 fixas — e das
**421** pastas pendentes, 371 só eram alcançáveis por quem adivinhasse parte do
caminho no filtro, enquanto o próprio cabeçalho já dizia que existiam.

A fila de arquivos, essa, já paginava desde o começo.

### Seis amarelos empilhados

Cada arquivo mostrava até seis candidatos, cada um com um `é esse` em amarelo
sólido. Isso diz que as seis opções pesam o mesmo — e a lista vem ordenada por
confiança justamente porque não pesam. Só o topo ficou sólido; o resto virou
contorno, igualmente clicável.

### Verde e vermelho ficam — e por quê

O score usa verde/âmbar/vermelho, que é uma **terceira** cor no sistema e viola
a regra da R1 na letra. Fica, pela mesma razão do selo de modo do player: aqui a
cor **significa** alguma coisa. 85% e 40% não são o mesmo tipo de decisão, e
achatar os dois em amarelo perderia informação pra ganhar consistência. Exceção
consciente, registrada.

### A luz apagando

Clicar em tocar trocava de tela num quadro. Agora escurece em três tempos, e a
ordem é o ponto:

    1. o fundo fecha em preto        0,26 s
    2. o quadro cresce de 0,965      0,42 s
    3. o cromo entra                 0,32 s, com 0,22 s de atraso

Entrar tudo junto faz parecer modal abrindo; nesta ordem parece sala
escurecendo. O cartão clicado afunda 3% antes disso — é o único retorno tátil
entre o clique e a sala.

A regra global de `prefers-reduced-motion` no topo do arquivo mata as três de
uma vez; não precisou de nada específico.

---

## 17. Canais ao vivo (R6)

O Odeon **sintoniza, não programa**. Uma fonte IPTV publica a lista (M3U) e a
grade (XMLTV); daqui pra frente isto é leitura.

### Canal não é obra, programa não é coleção

Três tabelas novas (`0013`) e **nada** encostou no grafo. São eixos diferentes:
o grafo descreve o que a obra É, a grade descreve QUANDO algo passa. Misturar
faria `work` carregar horário e `collection` carregar canal.

`programme.work_id` existe, anulável, sem lógica nenhuma: é o gancho pra quando a
grade apontar pra uma obra da sua biblioteca ("quer ver do começo?"). Custa uma
coluna hoje e evita uma migração dolorosa depois.

### A URL que o provedor escreve não é a URL de onde você está

O ErsatzTV publica os streams como `http://localhost:8409/iptv/channel/1.ts`.
Dentro do container do Odeon, `localhost` é o próprio container: o import
passaria e **todo canal falharia no play**. É a mesma armadilha do `10.0.2.2` no
emulador Android (§8e).

Medido de dentro da rede do compose:

    localhost:8409             ECONNREFUSED
    host.docker.internal:8409  ENOTFOUND     (Linux não provê por padrão)
    ersatztv:8409              ENOTFOUND     (outra rede compose)
    172.17.0.1:8409            HTTP 200      (gateway da bridge)

A regra, no espírito do CORS do §10b, é comparativa: **stream com host de
loopback é reescrito pro host da fonte** — que é, por definição, um host
alcançável, porque a lista acabou de ser baixada de lá. A porta do stream é
preservada; só o host muda. Host de verdade não é tocado.

A reescrita acontece **no import**, não no play: assim o que está no banco é o
que funciona, e quem lê não precisa conhecer a regra.

### Uma sessão por canal, não por usuário

No transcode sob demanda cada pessoa está num ponto diferente do arquivo, então
a sessão é por usuário. Ao vivo todos veem o mesmo instante — um ffmpeg por
espectador seria desperdício puro e ainda multiplicaria a banda puxada do
provedor. Verificado: dois `watch` no mesmo canal devolvem a mesma sessão.

E a saída é **janela deslizante** (`hls_list_size 6` + `delete_segments`), não a
playlist `event` que só cresce: transmissão não acaba, e uma playlist infinita
encheria o disco. Verificado: `MEDIA-SEQUENCE` avançou de 1 para 8 em 30s com a
playlist fixa em 6 e sete `.ts` no disco.

### O bug que quase não deu pra achar

`watch` respondia na hora, e o ffmpeg ainda estava conectando no provedor. A
primeira requisição de playlist caía em `wait_for_playlist` e ficava presa por
até 25s — mais do que o hls.js aguenta.

O sintoma era cruel: **nenhum erro em lugar nenhum**. Playlist e segmentos
respondiam 200 quando testados à mão, o MSE aceitava o codec, os segmentos
começavam em keyframe, o player sob demanda tocava com o mesmo código. O que
denunciou foi instrumentar os eventos do hls.js e ver que ele parava em
`MANIFEST_LOADING` e nunca chegava em `MANIFEST_PARSED`.

Todos os meus testes manuais rodavam *depois* de esperar, com a playlist já
pronta — e por isso passavam.

A correção: `watch` **só responde quando há o que tocar**. Custa ~5s do lado de
cá; do lado de lá era funcionar ou não.

### R6b — a arte, a espera e o lembrete

**A capa vem da sua biblioteca, porque o XMLTV não tem.** O `<icon>` do XMLTV é
só o logo do canal — programa não traz arte. Então `programme.work_id`, que
nasceu como gancho anulável no 0013, passou a ser preenchido.

O casamento é **conservador de propósito**, e o número explica por quê: casar só
por título acharia obra para 410 dos 899 programas, mas **734 títulos da
biblioteca são ambíguos** — em geral episódios que repetem o nome da série.
Escolher um no chute mostraria a capa errada, que é pior do que não mostrar
capa. Só liga quando o título é **único** entre as obras com arte, e o ano do
XMLTV derruba o casamento quando discorda.

Resultado medido: 229 dos 902 programas ligados, **7 dos 17 canais no ar com
arte**. "Sherlock" (a série, no ErsatzTV) não casa com "Sherlock Holmes" (o
filme, na biblioteca) — e é exatamente isso que se quer. A taxa sobe sozinha
conforme o acervo é identificado.

O scrim sobre a capa não é enfeite: o cartão existe pra dizer o que está
passando e quanto falta, e capa clara com texto por cima apaga as duas coisas.

**A espera passou a ser dita.** Desde que `watch` só responde com a playlist
pronta (~5s), a tela ficava muda o tempo todo. Agora o cartão que você clicou
mostra "pedindo o fluxo ao provedor…" — a frase diz o que está acontecendo, não
"carregando". Os outros cartões continuam clicáveis.

**Lembrete.** `programme_reminder` é por usuário, com FK `ON DELETE CASCADE`
para `programme`: a grade é regravada inteira a cada importação, então todo
lembrete morre junto. É deliberado — se o provedor reprogramou, o horário que
você agendou não existe mais; guardar por título sobreviveria, mas passaria a
avisar de uma reprise que ninguém pediu.

O vigia roda a cada 30s e marca `notified_at` **na mesma consulta que
seleciona** (`UPDATE ... RETURNING`), então duas passadas concorrentes não
avisam duas vezes — quem resolve a corrida é o banco, não um mutex no processo.
A janela é de 2 minutos à frente e 5 para trás; o limite de trás existe porque o
servidor pode ter ficado fora do ar, e avisar de algo que já acabou é pior que
não avisar.

Verificado de ponta a ponta com um lembrete real: programa às 05:56:32, aviso
carimbado às 05:54:48 — **1min44 de antecedência**.

O aviso vai pelo barramento SSE que já existia, e o componente vive no `App` e
não na aba: agendar não serviria de nada se o aviso só chegasse com a aba "ao
vivo" aberta. A permissão de notificação é pedida **no clique**, não na carga da
página — navegador esconde pedido feito sem gesto do usuário, e ninguém entende
por que o aviso nunca chega.

### R6c — dois defeitos que a verificação anterior não pegou

**O cromo não sumia.** Dois problemas distintos com a mesma cara:

- o player **ao vivo não tinha auto-hide nenhum** — omissão minha na R6, e os
  screenshots não denunciaram porque screenshot não espera;
- o player sob demanda escondia só `if (!paused)`, então quem pausasse ficava
  com a barra na tela para sempre.

Agora os dois somem depois de **3s sem mexer o mouse** e voltam ao primeiro
movimento, pausado ou não. Esconder pausado é escolha explícita: o botão grande
de tocar vive no palco e não some, então o caminho de volta continua visível.

**O modal do guia saía cortado.** `.modal-corpo` tinha `margin-top: -60px` pra
subir sobre a arte — e sem arte ele subia pra fora da caixa. Medido: modal em
`top: 335`, corpo em `top: 276`, título renderizando **59px acima da própria
borda**. O cabeçalho inteiro (selo do canal, título, botão de fechar) ficava
clipado.

Como 673 dos 902 programas não têm arte, esse era o caso **comum**, não a
exceção — e os meus dois testes anteriores calharam de abrir programas com
capa. A subida agora só vale sob `.com-arte`. Verificado abrindo 16 modais em
sequência: título dentro da caixa e botão de fechar presente em todos.

### R6d — por que o auto-hide "consertado" ainda não escondia

A correção anterior fazia a classe `.idle` entrar — e eu **verifiquei a classe,
não o efeito**. O cursor sumia (prova de que `.player.idle` casava) e a barra
ficava. Medindo a opacidade computada:

    classeIdle: true    cursor: none
    .player-top   opacity: 1   animationName: odeon-entrar-cromo   fill: both
    .player-scrim opacity: 1   animationName: odeon-entrar-cromo   fill: both

**Animação fica acima de declaração normal na cascata.** O
`animation-fill-mode: both` da entrada da sala escura (§16) mantinha o último
quadro (`opacity: 1`) grudado depois que a animação acabava, e isso anulava em
silêncio o `opacity: 0` do `.idle`. Duas features boas, escritas em fases
diferentes, brigando por uma propriedade.

`backwards` no lugar de `both` resolve: o primeiro quadro continua valendo
durante o atraso — a entrada é idêntica — e a propriedade volta pra cascata
quando a animação termina.

E um segundo defeito escondido atrás do primeiro: no player sob demanda o
cronômetro só nascia na primeira interação, então **quem abrisse um vídeo e não
mexesse em nada nunca via a barra sumir** — justamente quem só quer assistir.
Agora ele começa na montagem.

A lição, que já apareceu na R6 com o `MANIFEST_LOADING`: verificar o mecanismo
não é verificar o resultado. `classList.contains("idle")` era verdade e a tela
estava errada.

---

## 18. Legenda em arquivo, não só embutida

O Odeon lia legenda só de dentro do container, via `ffprobe`. Isso deixava a
maioria dos filmes sem legenda nenhuma — e era a diferença entre "o Jellyfin
mostra e o Odeon não".

Medido neste acervo antes de escrever qualquer linha:

    arquivos .srt/.ass/.sub no disco ....... 4.136
    filmes com faixa embutida .............. 135 de 635
    filmes SEM embutida que têm arquivo .... 348 de 400 (amostra)

E o efeito, medido em 25 filmes sorteados: **1 com legenda antes, 23 depois.**

### Onde procurar

Dois lugares, que é onde elas realmente estão:

- **irmãs na mesma pasta**, com o nome começando pelo do vídeo — o padrão YTS
  (`1917.2019.1080p...[YTS.MX].pt-BR.srt`);
- a subpasta **`Subs/`**, onde o nome é do idioma e não do filme
  (`Subs/Brazilian (Forced).por.srt`).

A busca acontece **no pedido, não no scan**. Jogar um `.srt` na pasta passa a
valer na hora, sem revarrer dezessete mil arquivos — e o custo é um `read_dir`
por requisição de plano.

### Índice negativo

Faixa em arquivo recebe índice **-1, -2, …**. O índice positivo é `0:s:N`, o que
o ffmpeg espera para faixa embutida; misturar os espaços faria uma externa de
índice 2 virar a terceira faixa do container na hora de extrair. Negativo não
colide com nada que o ffmpeg produza, e `e_externo()` é uma comparação.

### O que o nome do arquivo diz, e o que ele não diz

`descreve_arquivo` tira idioma e "forçada" do nome, e **não inventa**: sem
sufixo reconhecível, o idioma fica `None` e o rótulo vira "Legenda". Chutar
"inglês" porque a maioria é inglês seria mentir com cara de metadado — a mesma
regra do score do M1.

`read_dir` não garante ordem, então a lista é ordenada por nome antes de numerar:
índice que muda entre dois pedidos faria o player pedir a faixa errada.

### O que ainda não dá

**Queimar legenda em arquivo.** Queimar passa o índice pro filtro
`subtitles=si=N`, que conta faixas do container — índice negativo não existe pra
ele. O caminho é `subtitles=filename=`, e não está feito. Por isso o botão
"queimar" só aparece em faixa embutida; oferecer e falhar seria pior que não
oferecer.

### Os três pontinhos que "não faziam nada"

Duas coisas somadas, e a segunda escondeu a primeira.

**A regressão.** Na R4, ao trocar `COLLECTION_COLUMNS` pelo rollup único, o
patch casou cinco formatos de SQL e deixou o sexto — `collections_of`, a
consulta que o detalhe da obra usa pra listar as coleções dela. O SQL passou a
citar `agg.item_count` sem ter `agg` no `FROM`:

    error returned from database: missing FROM-clause entry for table "agg"

`GET /api/works/{id}` devolvia 500. Compila, porque o SQL é montado em runtime —
a mesma classe de armadilha do `sqlx::FromRow` do §14, e do mesmo jeito só
aparece quando aquela rota é chamada. Eu verifiquei a aba de coleções depois da
R4 e **nunca abri a gaveta de detalhe**.

**Por que parecia "nada".** O ramo de erro do `Details` devolvia
`<div class="drawer">` **sem** o `.drawer-backdrop` — e é o backdrop que tem
`position: fixed`. Sem ele a gaveta caía solta no fim do layout e sumia da tela.
Havia um erro; ninguém conseguia lê-lo. E durante o carregamento o componente
devolvia `null`, então o clique ficava sem resposta nenhuma.

Agora erro e carregamento usam a mesma moldura do estado normal.

**O que eu fiz depois de consertar,** porque uma consulta esquecida sugere
outras: varri as 37 rotas GET com ids reais. Todas em 200 — exceto
`/api/media/{id}/scrub` num arquivo sem folha de sprite, que é 404 correto (só
725 dos 17.498 têm).

## 19. R7 — a ficha vira cartaz

Última tela do fluxo principal com a cara pré-redesenho: a gaveta que abre nos
três pontinhos do cartão. 480px na direita, quatro seções de texto, três
formulários sempre abertos, nenhuma arte.

### O diagnóstico não era estético

Antes de desenhar, medi o que a rota devolve contra o que a tela usa.
`GET /api/works/{id}` responde com 23 campos; o `WorkDetail` do `api.ts`
declarava 14 e o componente consumia 8. Estavam sendo descartados:

| No JSON | Onde aparecia |
|---|---|
| `artwork` — pôster, backdrop, still | em lugar nenhum |
| `runtime_seconds` | em lugar nenhum |
| `files[]` — resolução, codecs, canais, tamanho, container, fps | em lugar nenhum, **e em nenhuma outra tela do Odeon** |
| `position_seconds` / `finished` | em lugar nenhum |
| `external_ids` | em lugar nenhum |

Consequência que importa mais que a pele: a única tela dedicada a *uma* obra
era a única de onde não dava pra assistir a ela. Um beco sem saída.

Zero mudança de backend nesta fase. Tudo já vinha no mesmo JSON — o
`WorkDetail` do TypeScript é que era um recorte, e um tipo que descreve menos
do que o servidor manda é uma tela que ninguém sabe que está incompleta.

### Formato: cartaz centrado

A gaveta lateral perdeu pra modal centrada de ~920px. O conteúdo é largo por
natureza (backdrop 16:9, faixa de elenco, duas colunas de grafo) e 480px
obrigavam tudo a virar coluna. Mesma família visual da modal do guia da R6b.

Não reaproveitei o `.drawer-backdrop`: ele empurra pra direita e o
`Servidor.tsx` continua usando ele. Classe nova, `.cartaz-fundo`.

O topo prefere `still` → `backdrop` → `poster`, a mesma ordem da R3 e pela
mesma razão: numa temporada o backdrop é a mesma imagem em todos os episódios.
O `--accent-work` fica no halo da caixa e na borda do pôster e em nada mais —
política da §12.

### A ficha técnica, e por que o degrau vem da largura

`1080p · h264 · ac3 5.1 · 2,6 GB · mkv`, em monoespaçada porque é medida, não
prosa.

O primeiro rótulo saiu errado na maquete: **818p**. O arquivo é 1920×818 — um
1080p em 2.35:1. Rotular pela altura diz que o arquivo é pior do que é, porque
a altura varia com a proporção e a largura não. Então o degrau vem da largura.

Só que aí 640×480 e 640×360 viraram os dois "640px", o que não é nome de nada.
Abaixo de HD a convenção sempre foi a altura — material SD é "480p", nunca
"640 de largura". A regra final tem as duas metades, e é por isso que
`resolucao()` inverte o critério em 1100px.

### A edição sai da leitura

Dois `<select>`, um campo de busca e um de texto livre moravam permanentes no
meio de uma superfície cuja função é mostrar uma obra. Foram pra trás de um
`✎ editar`, junto com os ✕ de remover — apagar também é edição. Quem abre a
ficha pra decidir se assiste não esbarra mais no editor do grafo.

### Detalhes que a implementação corrigiu

- **Produção agrupada por papel.** Um episódio de série traz onze produtores;
  onze linhas repetindo "PRODUÇÃO" é ruído. Os créditos já chegam ordenados
  por papel, então juntar vizinhos iguais basta.
- **Elenco em faixa horizontal.** Empilhado, doze rostos empurravam tags e
  coleções pra fora da tela.
- **A barra de rolagem do elenco.** A padrão do Firefox entra clara demais no
  fundo preto e lê como um elemento solto; virou dourado apagado sobre nada.
- **Sobrelinha do episódio.** A árvore de coleções que o componente já buscava
  dá o pai de cada temporada, então o cabeçalho diz
  "AS VISÕES DA RAVEN · TEMPORADA 3" e não só "Temporada 3".

### Verificação

Contra o acervo real: um filme (007, 1920×818, quatro papéis de equipe,
21 créditos) e um episódio com progresso salvo (`▸ continuar · faltam 10min`).
Clicar em ASSISTIR abre o player com `readyState 4` — o autoplay é bloqueado no
headless, o resto do caminho não. A 700px de largura a grade vira uma coluna e
nada estoura na horizontal.

## 20. R8 — a locadora

Aba nova (`experimentação`, nome provisório): a biblioteca vista como loja de
aluguel, com as obras em caixas de VHS ou DVD conforme o ano.

### A decisão que definiu a feature: o que é uma caixa

O acervo tem 17.498 registros, e 14.657 deles são episódios. Uma locadora com
14.657 fitas não é uma locadora, é um depósito — numa loja de verdade **uma
série é uma caixa**, não vinte e uma.

Com essa regra o acervo vira 635 filmes + 115 séries. E isso torna
`/api/library` — a listagem agrupada da R3, "uma entrada por série, uma por
obra avulsa" — a fonte exata. Backend novo: nenhum.

A segunda regra saiu da primeira: **só entra o que tem capa**. Ficam de fora
~11.900 registros (episódios avulsos, material de YouTube, não identificados).
Não é limitação do código; é o formato. Uma estante é feita de capas, e uma
caixa sem arte não é uma caixa. Sobram **600**: 96 fitas, 504 discos, 71 delas
caixas de coleção.

### VHS ou DVD

Corte em **1996**. O DVD chegou ao Brasil em 1998–99, mas a locadora só virou
de verdade depois de 2000; 1996 deixa a proporção certa — a prateleira de VHS
é o cantinho dos clássicos, não a loja inteira.

As duas caixas têm proporções reais e diferentes: VHS 104×200×28, DVD
130×184×15, caixa de coleção com 44 de lombada. O que separa as duas a três
metros de distância não é o tamanho, é a **lombada**: no DVD ela é plástico na
cor dominante da obra, no VHS é papel branco com texto preto.

### O 3D é CSS, e por que ele existe

Três faces por caixa — capa, lombada e topo — sem nenhuma biblioteca nova
(three.js custaria ~600 KB pra desenhar caixas de sapato).

Dois detalhes que custaram uma rodada cada:

- **`rotateY` positivo traz a aresta esquerda pra frente**, então a lombada
  mora à esquerda da capa. Com o sinal invertido ela fica atrás da arte e o 3D
  simplesmente some. A lombada gira sobre a própria aresta direita — o truque
  da capa de livro — o que dispensa calcular um `translateZ` por face.
- **A caixa na mão para em 164°, não em 180°.** Virada exatamente de costas, a
  lombada fica de perfil e o objeto volta a parecer um retângulo chapado.
  Dezesseis graus a menos deixam a espessura à vista.

E o 3D só se justifica por causa da **contracapa**: sem verso, a caixa seria um
pôster com sombra. O que vai atrás de uma caixa de verdade é exatamente o que a
R7 já sabia montar — sinopse, algumas cenas e a ficha técnica.

### O que a locadora descobriu sobre os dados

**Nenhuma das 115 séries tem sinopse.** `collection.overview` é nulo nas 115, e
o `external_ids` delas vem vazio: a coleção-série é um agrupamento que o
scanner cria e que o identificador **nunca enriquece** — ele trabalha em obras.
Isso deixaria 71 das 600 caixas com o verso em branco.

Não dá pra inventar sinopse. Mas a contracapa de um box set traz mesmo a
**lista de episódios**, e essa existe: a caixa de coleção mostra os títulos da
primeira temporada. Não é remendo — é o que aquela contracapa sempre teve.

Fica registrado como dívida: enriquecer `collection` de `kind='series'` no
identificador resolveria isso e também a aba de coleções.

Detalhe correlato: os episódios pendem das **temporadas**, não da série. Pedir
`/api/collections/{série}` devolve as temporadas em `children` e `items` vazio
— por isso o verso da caixa de coleção dá uma segunda volta na primeira
temporada pra ter as cenas.

### Vocabulário de gênero: dois idiomas no mesmo acervo

As estantes juntam vários rótulos crus porque há **dois** vocabulários: o
provider de filme responde em pt-BR ("Ficção científica", "Terror") e o de
série em inglês ("Sci-Fi & Fantasy", "Action & Adventure", "Kids"). Sem a
união, uma estante teria só filmes e a outra só séries. É o `tag_mode=any` do
M2 fazendo exatamente o que foi feito pra fazer.

Cada título fica numa estante só, e por isso a **ordem importa**: os gêneros
distintivos reivindicam primeiro. Com DRAMA na frente ele engoliria metade do
acervo e as outras estantes ficariam vazias.

### Custo

12 requisições em paralelo, 617 KB, uma vez por visita à aba. É muito pra um
app de celular e é pouco pra um servidor de mídia numa LAN; o alternativo seria
pedir 40 por estante, mas aí a atribuição exclusiva não fecharia e estantes
inteiras apareceriam vazias.

### Verificação

Contra o acervo real: 14 estantes montadas (600 caixas, 96 VHS, 71 coleções),
o balcão com Devoluções e Lançamentos, a contracapa de um filme (1408 — 1h52 ·
1080p · h264 · AAC estéreo · 1,8 GB · mov) e a de um box (Yellowstone — 5
temporadas · 53 episódios, com os títulos da 1ª temporada e três cenas de
episódios diferentes). `▸ assistir` chega ao player com `readyState 4`;
`▸ ver a série` cai na biblioteca filtrada pela coleção. A 700px o balcão vira
uma coluna e nada estoura na horizontal.

## 21. A série vira dona da própria identidade

A locadora (§20) esbarrou num buraco: nenhuma das 115 séries tinha sinopse.
Puxando o fio, o buraco era maior do que texto faltando.

### O diagnóstico

`collection` de `kind='series'` nasce em `apply_candidate` e nunca mais é
tocada. Nas 115 séries do acervo: `overview` NULL em 115, `external_ids` `'{}'`
em 115, `artwork` `'{}'` em 115. Não é bug de escrita — é ausência de código. O
identificador enriquece **obras**, e a série nunca é uma obra.

A consequência barata é a sinopse. A cara é outra:

**O pôster e o backdrop que o provider devolve para um episódio são os da
SÉRIE.** Arte por episódio existe e tem outro nome — é o `still`. Baixando
pôster e backdrop por obra, o acervo guardava a mesma imagem uma vez por
episódio:

```
18.004 arquivos  →  1.429 imagens distintas
2,19 GB          →  197 MB se guardasse uma cópia de cada
```

Uma das imagens estava salva **553 vezes**. Além do disco, é um download por
episódio contra o TMDB numa identificação completa — 8.471 requisições para
buscar 115 imagens.

E a cor dominante, que sai da decodificação do pôster, era recalculada em cada
uma dessas cópias.

### A correção

Uma causa, uma correção: **a série passa a ser dona do arquivo**.

- `0015_serie_dona_da_arte.sql` — `collection.dominant_color`. A cor mora onde
  mora o pôster de que ela sai.
- `ensure_serie()` substitui a chamada solta a `ensure_collection` no caminho
  do episódio. Ela garante a coleção **e a enriquece**: sinopse, `external_ids`
  e arte, baixada uma vez com o nome da coleção.
- A ordem em `apply_candidate` mudou: a série é resolvida **antes** do bloco de
  artwork, porque é dela que a arte vem. Episódio não baixa mais pôster nem
  backdrop — herda o caminho e a cor. O `still` continua por obra, porque
  aquele é genuinamente por episódio.
- A guarda de rede é precisa: só busca o que falta *e* que o candidato tem como
  preencher. Sem ela seriam dois downloads por episódio para reescrever sempre
  a mesma coisa.

### O reparo do que já existia

`POST /api/maintenance/repair-series`, com `dry_run` ligado por padrão — mesma
convenção do reparo de títulos: contar é inofensivo, reescrever 8.343 obras não
é.

Um `GET /tv/{id}` por série (114 chamadas, não 8.471), a arte baixada com o
nome da coleção, e os episódios repontados para o arquivo da série.

**Resultado medido:** 114 séries enriquecidas, 1 sem suporte (a única de
AniList — o reparo é TMDB, como o de títulos), 0 falhas, 8.343 obras
repontadas, e 16.686 arquivos (2,00 GB) que deixaram de ter dono.

A rota é **retomável de propósito**: o `WHERE` só pega o que ainda falta. Isso
deixou de ser detalhe quando a primeira execução foi cancelada junto com o
cliente HTTP aos 82/115 — rodar de novo continuou de onde parou. (Que um reparo
de vários minutos ainda seja uma requisição síncrona é dívida conhecida; o
`repair-episode-titles` tem a mesma forma.)

### O que foi verificado, e como

Não bastava ver a sinopse aparecer — 8.343 obras tiveram o `artwork` reescrito,
e um caminho errado apagaria a arte da metade do acervo. As duas checagens que
importam:

1. **Integridade referencial contra o disco.** Toda referência do banco
   (`work.artwork`, `collection.artwork`, `person.image_path`) conferida contra
   o `ls` do diretório: 16.317 referências, **0 quebradas**. Antes disso,
   inventário de *todas* as colunas do schema que poderiam apontar para um
   arquivo — `channel.logo_url` é URL remota, `scrub_sprite` mora em outro
   diretório, `match_candidate.*_url` são URLs do provider.
2. **O caminho novo, não só o reparo.** Um episódio foi resetado e
   reidentificado: herdou o pôster da série e criou **zero** arquivos novos.
   Verificar só o backfill teria deixado a regressão viva na próxima varredura.

Na tela: 612 capas na locadora, **0 quebradas**.

### Efeito colateral no verso da caixa

A lista de episódios do §20 existia como *substituta* da sinopse. Com 114 das
115 séries ganhando sinopse, ela viraria código morto — então passou a aparecer
**junto**, que é o que um box tem atrás de verdade. E a sinopse passou a cortar
por linha (`line-clamp`) em vez de por altura: `overflow: hidden` num bloco de
texto corta no meio de uma linha e parece defeito de renderização.

### O que ficou de fora

Os 16.686 arquivos órfãos **não são apagados** pelo reparo. Apagar arquivo é
decisão de quem administra, e `POST /api/maintenance/artwork-orfao` faz isso
separado — também com `dry_run` por padrão, e recontando as referências na
hora em vez de confiar num número que o reparo calculou antes.

## 22. R10 — o clique certo em cada lugar

Dois gestos estavam trocados na biblioteca.

**Clicar no cartão tocava o filme.** Começar um filme é uma decisão, e a
decisão precisa da sinopse na frente. Pior: era irreversível de graça — um
clique errado acendia uma sessão de transcode.

**Os três pontinhos abriam a ficha.** Que é onde o cartão inteiro deveria
levar. E o resultado é que não havia lugar nenhum para *"esse arquivo está
identificado errado"* ou *"apaga isso daqui"* — a operação sobre o **registro**
e sobre o **arquivo** simplesmente não tinha porta.

Agora: **cartão → a ficha** (o cartaz da §19, que já tem `▸ assistir`);
**⋯ → gerenciar**.

Vale só para a biblioteca, que foi o que estava trocado. O painel e a locadora
continuam como estão: lá a arte é o convite, e o gesto de "quero este" já é
explícito.

### O que a gaveta de gerenciar faz

Quase tudo já existia no backend e não tinha tela fora da fila de revisão:

- **Arquivo** — o caminho completo, que quebra em vez de truncar (quem abre
  isto quer o caminho inteiro) e copia com um clique, mais a ficha técnica.
- **Identificação** — o estado, a confiança, os ids do provider, e a busca
  manual: é `POST /api/works/{id}/search`, a mesma da fila de revisão. A
  diferença é o ponto de partida — ali a obra nunca foi identificada, aqui ela
  foi e alguém discorda.
- **Corrigir o que o parser entendeu** — `setParse`/`clearParse`, que sobrevive
  a nova varredura e a nova identificação porque é decisão humana.
- **Zona de risco** — ignorar e apagar do disco.

### Apagar do disco: o que a configuração impedia

`/media` está montado **`:ro`** no `docker-compose.yml`. Nenhum código faria
isso funcionar.

Então `GET /api/storage` responde **testando escrita de verdade** — cria e
apaga um arquivo em cada raiz. Não dá pra deduzir da configuração: `:ro` no
compose, permissão do usuário do container e filesystem de rede somente-leitura
são três motivos diferentes para o mesmo "não dá", e só a tentativa distingue.
É a mesma política do `hwaccel`, que testa o encoder em vez de acreditar no
`ffmpeg -encoders`.

Com isso o botão nasce desabilitado **com o motivo escrito embaixo** em vez de
existir e falhar quando clicado.

### A ordem da exclusão

**Disco primeiro, banco depois.** Se um arquivo se recusa a sumir, nada é
removido do catálogo e o erro sobe com o caminho. O contrário deixaria arquivos
invisíveis ocupando disco — o banco afirmando que apagou algo que continua lá.

E `media_file.work_id` é `ON DELETE SET NULL`, **não** cascade: apagar só a
obra deixaria o arquivo pendurado no banco sem dono. Os dois vão juntos, na
mesma transação.

### "Ignorar" existe porque "remover do catálogo" não funciona

Tirar a obra do banco sem apagar o arquivo é um botão que se desfaz sozinho: a
próxima varredura acha o arquivo e recria a obra. Então a alternativa
não-destrutiva é `match_state = 'ignored'` — a linha fica, marcada, e a
varredura não a ressuscita.

Rota própria, `POST /api/works/{id}/ignore`, e não o `bulk_state`: aquele
recusa obras `confirmed` de propósito, porque em lote desfazer uma
identificação boa por engano é caro. Aqui é uma obra, escolhida a dedo, com o
botão embaixo do nome dela.

### Verificação de um endpoint destrutivo

Não dá pra testar exclusão clicando no acervo de alguém. Uma obra descartável
foi criada apontando para um arquivo em `/cache`:

1. **Sucesso** — arquivo gravável: sumiu do disco, `work` e `media_file` saíram
   do banco, 2.048 bytes relatados.
2. **Recusa** — alvo impossível de apagar: `HTTP 400`, `"não apaguei
   `/media`: Is a directory. Nada foi removido do catálogo."`, e as duas linhas
   **intactas** no banco.

O segundo caso é o que importa: sem ele, a garantia de "disco primeiro" seria
uma frase no comentário. Os dados de teste foram removidos depois.

Detalhe pego na tela: o status `probed` — que é o estado **normal** de 17.498
dos 17.503 arquivos — aparecia pintado de vermelho como se fosse erro, porque
a condição era `status !== "ok"` e nenhum arquivo tem status `"ok"`.

### Adendo à R10: a montagem virou gravável

Decisão de quem administra este servidor, tomada depois de ver o botão
desabilitado: as três montagens de mídia perderam o `:ro`.

O que se perdeu é real e vale escrever: o `:ro` era uma garantia estrutural —
nenhum bug meu e nenhuma sessão de admin vazada alcançava a mídia. Isso agora
depende de código, não de montagem: só admin apaga, a confirmação é por
digitação com a lista de arquivos na frente, e a ordem disco-antes-do-banco já
foi verificada nos dois desfechos.

Voltar atrás é acrescentar `:ro` de novo e `docker compose up -d` — e nada
quebra, porque `GET /api/storage` **pergunta ao disco** em vez de assumir. Foi
exatamente pra isso que a checagem foi feita por escrita de teste.

Verificado depois da troca: `pode_apagar: true`, o botão habilitado com
"Apaga 1 arquivo (0,3 GB) e o registro. Não tem volta.", a confirmação listando
o caminho real, e o botão final travado até se digitar `apagar`. A verificação
parou aí de propósito — o acervo é do usuário.

### Adendo: a faxina do artwork órfão

Executada depois da §21, com a autorização de quem administra: 16.686 arquivos,
**2,00 GB**. `/cache/artwork` foi de 2,7 GB para 778 MB.

A conferência que importa veio depois: 16.317 referências no banco, 16.317
arquivos em disco, **casamento exato** — zero referências quebradas e zero
órfãos restantes.

## 23. R11 — a caixa como objeto (experimento)

Pedido como teste, e escrito pra ser fácil de tirar: tudo vive em
`Locadora.tsx` e no bloco `a caixa na mão (R11)` do CSS.

Três movimentos substituem o "abre uma modal já virada de costas".

### 1. A caixa vem da estante

Antes ela nascia pronta no centro com um giro de 164°. A informação chegava,
mas o objeto não: era uma modal, não uma caixa que alguém pegou.

Agora ela voa do lugar exato de onde foi pega, pela técnica do **FLIP** — o
elemento já está no destino, e o que se anima é a diferença entre onde ele
estava e onde ele está. O clique manda o `getBoundingClientRect()` do cartão da
prateleira junto.

Dois detalhes que custaram uma rodada cada:

- **Medir antes de animar.** `getBoundingClientRect` devolve o retângulo já
  transformado. Como a classe da animação entra no primeiro render, a medição
  via a caixa deslocada pela própria animação e concluía que o deslocamento era
  zero — no StrictMode, que roda o efeito duas vezes, isso acontecia sempre. A
  correção é desligar a animação, forçar o reflow, medir, religar.
- **Dois elementos, não um.** Translação e rotação 3D no mesmo `transform`
  brigam: a translação passaria a acontecer no eixo já girado. O invólucro
  cuida de posição e escala, a caixa cuida da rotação.

### 2. Girar na mão

Arrastar gira em dois eixos (0,5°/px na horizontal, uma volta inteira em
~720px). Passando dos 90° aparece a contracapa, que já existia.

`backface-visibility: hidden` nas faces — sem isso a capa aparece espelhada do
outro lado, e o efeito vira papel em vez de caixa.

O limiar de 6px separa "arrastei pra ver o outro lado" de "cliquei na lombada".
Sem ele, todo giro que começasse na lombada abriria a caixa no fim do gesto.

### 3. A lombada é o play

Numa locadora era pela lombada que se puxava a fita da prateleira. Aqui é por
ela que o filme começa: clicar abre a caixa — a capa gira na dobradiça, o
interior aparece, e a mídia sai na direção de quem olha. O disco gira ao sair
porque disco gira; a fita não, porque fita não, e é essa diferença que faz o
olho reconhecer qual dos dois é.

Uma aresta de 40px não é um alvo que alguém procura sozinho, então ela acende
no hover e o rodapé diz o que fazer.

### Três defeitos que só a verificação achou

1. **Fechar no meio da abertura não cancelava a reprodução.** Apertar Esc
   fechava a caixa e o filme começava mesmo assim, um segundo depois — o
   `setTimeout` continuava vivo. Agora o relógio mora num ref e a limpeza do
   efeito o cancela.
2. **A lombada ficava inerte até o `api.detail` voltar**, e nesse meio tempo o
   clique sumia sem resposta. A condição de poder abrir passou a sair do
   `media_file_id`, que a estante já conhece; a espera pelo detalhe acontece
   **com a caixa aberta na tela**, não antes de qualquer resposta.
3. **`animation-fill-mode: both` de novo.** O véu escuro do fim tinha `from`
   transparente, e `both` aplica o primeiro keyframe *durante o atraso* — a
   locadora aparecia acesa atrás da caixa a animação inteira. Virou `forwards`
   e sem `from`. Terceira vez que este mesmo `both` morde neste projeto (§13,
   §16).

### Como se verifica uma animação

Print não serve, e capturar em tempo real também não: cada screenshot do
Marionette leva centenas de milissegundos, então os quadros sairiam espaçados
de forma imprevisível e nunca nos instantes que interessam.

`scratchpad/filmar.py` para o relógio: `document.getAnimations()` devolve as
animações em curso, e pausando todas e escrevendo `currentTime` a página vira
um quadro congelado em qualquer milissegundo escolhido — determinístico e
reprodutível.

Com uma correção que a primeira versão exigiu: **um disparo por quadro**. A
interface tem uma máquina de estados em `setTimeout` que corre no tempo de
parede e troca de fase enquanto os screenshots ainda estão sendo tirados; do
terceiro quadro em diante não havia mais animação nenhuma pra congelar. E cada
disparo passou a reportar o que aconteceu, porque um disparo que falha em
silêncio vira um quadro em branco no meio da tira sem ninguém saber por quê.

A filmagem usou caixas de **coleção** de propósito: a lombada de um box leva
pra biblioteca, então dezenas de repetições não acendem sessões de transcode no
acervo de ninguém. O caminho do filme de verdade foi verificado uma vez, no
fim: 1408 abriu o player 1,66s depois do clique na lombada.

### Adendo à R11: três correções, e uma delas expôs um erro de geometria

**1. O lado que abre é o direito.** A dobradiça de uma caixa é a lombada, à
esquerda; o lado por onde ela abre é o oposto. O botão estava na lombada — ou
seja, na dobradiça.

Agora existe uma face `.abertura`, com a fresta entre as duas metades, e é ela
que aciona. A pose de mão inverteu o sinal do giro (`POSE.y` de +26 para −24)
pra que esse lado fique à vista: na estante o giro é positivo, porque lá o que
se lê é a lombada; na mão é negativo, porque é a abertura que se clica.

**2. Arrastar não girava — o navegador roubava o gesto.** O `pointermove`
nunca chegava porque o Firefox iniciava o próprio arrasto de imagem no primeiro
pixel: girar a caixa virava arrastar uma miniatura da capa. Faltavam
`draggable={false}`, `user-select: none`, `touch-action: none` e
`pointer-events: none` na capa. O `setPointerCapture` também passou a ser
tolerante a falha — ele melhora o gesto que sai do elemento, mas o giro não
depende dele.

**3. A primeira caixa da fileira era recortada no hover.** E a causa é uma
regra de CSS que eu tinha escrito errado com confiança: **`overflow-y: visible`
não vale ali**. Quando um dos eixos deixa de ser `visible`, o outro computa
como `auto` — e `overflow-x` precisa ser `auto` porque a fileira rola. Ou seja,
a fileira sempre recortou nos quatro lados.

Medido: no hover a caixa sobe 30px e vem 90px na direção de quem olha, e o
`translateZ` a afasta do centro da perspectiva — a primeira andava 14px para a
esquerda, para x=34, contra a fileira começando em x=48. Resolvido com padding
dentro da área recortada (46px em cima, 30px nas laterais).

**O erro que apareceu no caminho.** Ao tentar esconder a face que está de
costas, `backface-visibility: hidden` escondia a errada. O motivo: o sinal do
giro de uma face decide para que lado ela **olha**, e as três laterais estavam
com a normal apontando para *dentro* da caixa — eram desenhadas de costas em
todas as poses, desde a R8. Enquanto nada dependia disso, ninguém viu; na hora
em que passou a depender, a lombada atravessava a capa e a caixa aparecia com
lombada dos dois lados.

Além do sinal, faltava `translateZ(--d / 2)`: girada sobre a própria aresta, a
face nasce entre 0 e −d, quando a caixa vai de −d/2 a +d/2 — cada lateral
estava meia espessura fora do lugar.

Conferido nas duas poses depois da correção: na estante as lombadas continuam
à esquerda, com o título legível; na mão só a abertura aparece.

### Adendo à R11: a caixa na mão cresceu

Ela é o objeto que a pessoa está olhando — precisa dar pra ler a contracapa e
pra mirar a abertura sem perseguir uma aresta de 30px.

O que limitava não era a janela, era a **dica e o botão empilhados embaixo**,
que comiam ~90px de altura e cobravam isso da caixa. Ancorados no rodapé do
overlay, fora do fluxo, a caixa passou a usar a janela inteira: de `58vh` para
`80vh` no DVD e `84vh` no VHS.

A largura e a espessura continuam saindo da proporção real do formato (DVD
135:190, VHS 104:191), então crescer não deforma — e a espessura acompanha,
senão um DVD grande viraria uma placa.

Medido nas duas telas, sem corte em nenhuma:

| janela | caixa | abertura na tela |
|---|---|---|
| 1280×734 | 383×605 | 41px |
| 1920×994 | 510×813 | 53px |

O alvo do clique era de 34px; agora vai de 41px a 53px conforme a tela.

### Adendo à R11: a mídia vira o segundo gesto (e o clique que não funcionava)

**O clique na abertura não funcionava — e a causa é o próprio giro.**

O `pointerdown` chama `setPointerCapture` na caixa, pra que o gesto não se
perca quando o ponteiro sai do elemento. Só que, com o ponteiro capturado, o
navegador **redireciona o `click` para quem capturou**: o `onClick` da abertura
nunca disparava, porque o clique chegava na caixa.

O sintoma enganava: um clique sintético despachado direto no elemento
funcionava, e só o mouse de verdade falhava — exatamente a diferença entre
disparar o evento e deixar o navegador gerá-lo. A correção é resolver o toque
no `pointerup`, olhando **onde o dedo desceu**, que a captura não altera. O
`onClick` continua lá como caminho redundante; ambos são idempotentes porque
`abrir()` e `tocar()` conferem a fase antes de agir.

**A caixa abre e entrega a mídia; o filme é um segundo gesto.**

Antes o filme começava no fim da animação de abertura — o disco aparecia por
meio segundo e o player o engolia. Renderizar um objeto 3D pra ele ser visto
por 500ms não é renderizar objeto nenhum.

Agora a abertura termina com o disco (ou a fita) no centro, e o play mora no
meio dele. São dois gestos porque são duas decisões: abrir a caixa e pôr pra
rodar.

- **O disco tem duas caras**, e as duas dizem coisas diferentes: o lado
  impresso com a arte e o lado de dados com a iridescência. Girar mostra os
  dois — é o motivo de valer a pena ser 3D.
- **A fita tem seis faces.** A espessura é 13% da largura, que é a proporção de
  um VHS (187 × 103 × 25 mm). Sem ela o objeto é um retângulo e o giro não
  convence.
- O alvo do play vai em **cada face**, com `backface-visibility` cuidando pra
  que só apareça o da cara que encara.

**O palco é irmão da caixa, não filho.** Filho, a mídia orbitaria junto com o
giro da caixa em vez de girar em torno do próprio centro.

E ele é **flex, não grid**: num grid o item é clampado pela área, e a área é do
tamanho da caixa. A fita, que é mais larga que uma caixa de VHS, saía espremida
de 455px para 345px — o objeto ficava menor que o recipiente de onde tinha
acabado de sair.

Verificado ponta a ponta com um filme real: o toque na abertura abre, o disco
fica no palco **sem tocar nada**, e o clique no centro leva ao player.

### Adendo à R11: o disco que sumia — e o método que falhou junto

Sintoma: clicar na abertura abria a caixa, o disco aparecia, e então sumia
deixando a tela preta.

**A causa foi de estrutura, não de CSS.** O palco da mídia acabou *dentro* da
`.caixa`, e não irmão dela — o comentário no código dizia "irmão", o JSX dizia
outra coisa. Quando a caixa recua para `opacity: 0.18` na fase da mídia, ela
leva o disco junto: disco a 18% sobre um véu quase preto é disco invisível.

Confirmado sem chute nenhum, perguntando ao navegador a cadeia de ancestrais:

```
midia3d:1 > palco-midia:1 > caixa:0.18 > voo:1 > mao-fundo:1
```

**Por que eu não tinha visto antes.** Três verificações passaram por cima
disso, e o motivo é constrangedor: a opacidade da caixa recuada tem transição
de **0,4s**, e eu conferia 300ms depois de a fase mudar. Fotografei o disco
sempre no meio do desvanecimento, quando ele ainda estava quase opaco. A regra
que sobra: quando o que se verifica tem transição, o instante de medir é
**depois** dela, não "logo após o evento".

**E o método também falhou.** Os testes com `elemento.click()` e
`PointerEvent` despachado à mão diziam que estava tudo certo, porque não passam
por hit-testing, não respeitam captura de ponteiro e não geram o `click` que o
navegador sintetiza. Foi preciso `WebDriver:PerformActions` — que injeta input
no nível do navegador — pra reproduzir o clique de verdade.
`scratchpad/clicar.py` ficou com isso.

**A correção trouxe uma segunda de brinde.** Movido o palco para fora da
caixa, os handlers de ponteiro continuavam **na caixa** — e nada do que
acontece num irmão passa por ela. Arrastar o disco não girava e o clique no
centro não tocava. Foram para `.voo`, o ancestral comum dos dois.

Verificado ponta a ponta depois, e no instante certo: disco em opacidade 1,
nada tocando antes do clique, o disco girando ao arrastar, e o player abrindo
ao clicar no centro. A fita idem, com as seis faces e 605px de largura.

### Adendo à R11: guardar a caixa, e tamanho que acompanha a janela

**Clicar fora do disco guarda a caixa.** A mídia volta pra dentro, a capa
fecha, e a caixa fica de novo na mão pra girar — é o inverso exato de abrir,
não um atalho pra fechar a locadora. Fechar tudo continua sendo o clique no
vazio **com a caixa fechada**, ou o Esc.

Isso obrigou uma troca que já estava pedindo pra acontecer: **a capa passou de
keyframe para transição**. Com keyframe a volta teria que ser escrita à mão, e
`animation-fill-mode` já mordeu este projeto três vezes (§13, §16, §23).
Transição não tem fill-mode: abrir e fechar são o mesmo caminho em sentidos
opostos, e um booleano decide a direção. A saída da mídia foi junto, pela mesma
razão — daí a classe `assentada`, que desliga a transição quando ela já chegou,
senão cada pixel de arrasto ficaria 0,7s atrasado.

**Tamanho: uma variável manda em tudo, inclusive no texto.**

A regra é `--h` (ou `--tam`, na mídia) definido com `clamp(piso, vh, teto)`, e
**todo o resto derivado dele**:

- geometria por `calc()` sobre a proporção real do formato — DVD 135:190, VHS
  104:191, cassete 187:103;
- tipografia por `font-size: calc(var(--h) / 40)` no elemento, e os filhos em
  `em`.

`clamp` e não `min` porque faltava o **piso**: numa janela baixa a caixa
encolhia até a contracapa ficar ilegível. E `vh` porque a caixa é alta — numa
janela larga e baixa, largura não é o que falta.

O `em` é o que faz a coisa toda ser um objeto e não um layout: quando a caixa
cresce, o texto impresso nela cresce junto, como numa ampliação da capa de
verdade. Em `px`, uma caixa grande ficava com letra de caixa pequena.

Medido nas duas telas — tudo cresce no mesmo passo (×1,37):

| | 1280×734 | 1920×994 |
|---|---|---|
| caixa (altura) | 621px | 851px |
| texto da contracapa | 15,0px | 20,4px |
| título do verso | 26,3px | 35,7px |
| lombada | 33,2px | 45,0px |
| disco | 525px | 717px |
| seta do play | 41,4px | 56,0px |

### Adendo à R11: os objetos, o fundo da caixa, e o arrasto que fechava

**Arrastar fechava a caixa.** O `click` do navegador é disparado no **ancestral
comum** entre onde o dedo desceu e onde subiu — então girar o disco e soltar
fora dele gerava um clique em `.mao-fundo`, e o "clicar fora guarda" disparava
no meio do gesto. A decisão saiu do `click` e foi para o par
`pointerdown`/`pointerup`, olhando **onde desceu** e **quanto andou**. É a
segunda vez que o `click` mente nesta tela; a primeira foi a captura de
ponteiro redirecionando o alvo.

**A caixa não tinha fundo.** Só `topo`. Bastava arrastar pra cima pra olhar por
baixo e ver o interior por um buraco. A face de baixo é o espelho da de cima,
com a normal apontando pra baixo (`rotateX(-90deg)`).

**Os objetos.** Um disco não é um círculo com gradiente. O que o olho
reconhece são cinco camadas, e agora cada uma é uma:

1. a **iridescência**, que não é um arco-íris uniforme — ela é forte na área
   gravada e some perto do centro, onde não há trilha. Sem essa segunda camada
   o disco vira CD de desenho animado, colorido do furo à borda;
2. as **trilhas**: anéis concêntricos de meio pixel, que é o que produz a
   difração — e o que faz a superfície parecer metálica em vez de pintada;
3. o **lustro**: duas faixas especulares opostas, como um tubo de luz no teto;
4. o **anel transparente** e o **anel de empilhamento**, o degrau em relevo que
   impede dois discos de colarem;
5. o **furo**, com a sombra da espessura.

Na cara impressa a arte é serigrafada — dessaturada, mais clara que a capa, e
com verniz por cima.

Na fita, o que a torna reconhecível de relance: corpo preto fosco com textura,
a **janela** com os dois carretéis **desiguais** (um cheio, um quase vazio — é
essa assimetria que diz "parou no meio"), doze dentes no cubo, os quatro
parafusos, a lingueta de proteção no verso, e a **tampa** na aresta da frente,
com a linha da dobradiça e o plástico mais liso que o corpo.

Doze dentes, não quatro: menos que isso vira ponteiro de relógio, mais vira
cinza.

## 24. R12 — o que estava quebrado e ninguém via

Uma varredura de saúde no servidor rodando, sem tela nova em mente. Achou três
coisas, e duas eram graves.

### 1. O guia ao vivo ia secar em 38 horas

A grade cobria até 2026-08-04 01:56. Passado esse prazo a aba "ao vivo" fica
sem programa nenhum — sem erro, sem aviso, sem nada. A importação sempre foi
**manual**, e a última tinha sido feita à mão doze horas antes.

`live::vigiar_grade` reimporta sozinho. Duas condições disparam, por motivos
diferentes:

- **cobertura futura abaixo de 24h** — impede o guia de secar, qualquer que
  seja o tamanho da grade que o provedor publica;
- **última importação há mais de 6h** — traz reprogramação. O provedor muda
  horário sem mudar o fim da grade, e sem isto o Odeon mostraria a grade velha
  por dois dias.

Mais um piso de 55 minutos entre importações da mesma fonte: um provedor que só
publique 12h de grade cairia na primeira condição a cada passada, e o vigia
viraria um laço de download.

### 2. Toda importação apagava os lembretes

Este é o pior, e só apareceu porque o item 1 ia fazê-lo acontecer o tempo todo.

`gravar_grade` apaga a grade da fonte e regrava — a decisão está documentada e
continua certa. Mas `programme_reminder.programme_id` é `ON DELETE CASCADE`:
**cada importação destruía silenciosamente todos os agendamentos**. Com
importação manual e rara, passou despercebido; com o vigia, o "me avisa quando
começar" do §17 deixaria de existir.

A identidade de um programa entre duas importações não é o `id` — que é serial
e se renova — é o trio **canal + horário + título**. Os lembretes são salvos
numa tabela temporária antes do DELETE e reatados por esse trio depois do
INSERT.

Programa que o provedor moveu de horário perde o lembrete, e perder é o certo:
o que a pessoa agendou não existe mais naquele horário.

Verificado com um lembrete de verdade: o programa foi de `id 2919` para
`id 3845` na reimportação, e o lembrete acompanhou.

### 3. Cinco arquivos que nunca vão tocar

2,89 GB. Quatro deles têm **zeros no início e no meio, dados no fim** — a
assinatura de download interrompido ou de alocação perdida no sistema de
arquivos; o `ffprobe` recusa com `EBML header parsing failed`.

O quinto, `Medabots 75.mp4`, começa com `%PDF-1.4`. É um PDF com extensão de
vídeo.

Nada disso é bug do Odeon — é o acervo. Mas **não aparecia em lugar nenhum**, e
o que não aparece ninguém conserta.

### O painel de saúde

`GET /api/diagnostico` e um bloco no topo da gaveta `Servidor`.

Regra do bloco: **só mostra o que está torto**. Um painel que repete "0 erros"
em cinco linhas ensina a não ser lido — e aí, no dia em que houver um erro, ele
também não será lido. Linha limpa some; linha suja fica, com o número, o motivo
e a lista de arquivos.

`/api/diagnostico` e não `/api/health`: aquele já existe e é o *liveness*,
responde sem autenticação e serve pra saber se o processo está de pé. São
perguntas diferentes — e foi um pânico no boot (`Overlapping method route`) que
avisou, não uma revisão minha.

Duas armadilhas de runtime no caminho, ambas do mesmo tipo (compila, quebra
depois): a rota duplicada, e `extract(epoch ...)`, que devolve `NUMERIC` no
Postgres 14+ e não `FLOAT8` — sem o cast explícito é 500 na hora da chamada.

O primeiro número que o painel mostrou já provou o item 1: **47h de grade**,
contra as 38h medidas antes. O vigia tinha reimportado sozinho no boot.

## 25. R13 — a ilha de transmissão, e o Odeon vira emissora

A aba ao vivo deixou de ser uma lista de canais e virou a mesa de quem opera a
emissora: **o que está no ar** → **para onde mudar** → **o que vem**.

E o Odeon deixou de só sintonizar.

### Canais que ele mesmo programa

Até aqui o §17 valia inteiro: uma fonte publica canais e grade, e daqui pra
frente é leitura. `live::emissora` inverte isso — três canais (Odeon 1,
Corujão, Matinê) que não existem em lugar nenhum: são **uma função da data
sobre a biblioteca**.

Três consequências que valem mais que o código:

1. **Não há stream.** Sintonizar é tocar o arquivo no offset que o relógio
   manda, e o M6 faz isso desde sempre (`?start=`). Nenhum ffmpeg a mais,
   nenhum canal "no ar" gastando CPU sem ninguém assistindo.
2. **Não há tabela.** A grade não é gravada, é recalculada. Duas chamadas no
   mesmo dia devolvem a mesma programação, em qualquer aparelho, sem nada pra
   sincronizar nem pra expirar. A ordem sai de `md5(dia || canal || id)` **no
   banco** — determinística sem trazer o acervo pra memória.
3. **Não há daemon.** O vigia do §24 existe porque grade de terceiro seca;
   esta não seca nunca.

Medido: 9.018 obras tocáveis, 4.929 horas — 205 dias sem repetir título.

**A âncora é a meia-noite local, não "agora".** A primeira versão montava a
programação a partir de `agora − 45min`; o efeito é que mover o relógio movia a
grade junto e **nenhum programa jamais virava**. Parecia funcionar porque a
tela só era olhada uma vez — foi rodar a maquete em três horários que revelou.
Como o Brasil não tem horário de verão desde 2019, um deslocamento fixo
(`ODEON_TZ_OFFSET`, padrão −3) resolve sem trazer a `chrono-tz` inteira.

### Um player só, e por quê

Canal IPTV é stream remoto; canal da casa é arquivo local num offset. Parece
que pedem players diferentes — e pediriam, se a decisão parasse aí. Mas os dois
viram **uma playlist HLS**, que é o que o player consome.

Sem essa unificação o zapeamento funcionaria em três dos vinte canais, o que é
pior do que não ter zapeamento. E "ver desde o início" cai de graça: é a mesma
chamada com offset zero.

### Ver desde o início

O stream está no meio do filme, mas o arquivo é seu. O botão só aparece quando
o programa no ar casou com uma obra da biblioteca — **10 dos 17 canais**. Nos
outros ele não existe, em vez de existir e falhar.

### O relógio não passa pelo React

A agulha do "agora" é escrita como propriedade CSS (`--agora`) num elemento só,
a cada 250ms. Em estado do React ela re-renderizaria vinte pistas e noventa e
três blocos quatro vezes por segundo pra mover uma linha de dois pixels.

250ms e não `requestAnimationFrame`: a agulha percorre cinco horas em alguns
pixels por minuto — a 60fps, 59 de cada 60 quadros escreveriam o mesmo valor e
a única diferença seria a bateria.

O React só é acordado quando o conteúdo muda de verdade: ao cruzar a fronteira
de um programa. Aí o herói e os cartões repintam.

### Cor e movimento

O **vermelho de transmissão** é exceção consciente à política do §12 — como o
verde/vermelho da confiança na revisão. Universal em emissora, e usado só no
"NO AR" e na pista em foco: marcando tudo que está no ar, vinte pistas acendem
juntas e o vermelho para de significar alguma coisa.

No `prefers-reduced-motion` **a agulha continua andando** (é informação, não
enfeite); o chuvisco e o pulso do ponto vermelho é que somem.

### O que foi verificado

Com os canais reais e a grade real: 20 pistas (3 da casa + 17 IPTV), 93 blocos,
a agulha andando (`--agora` 0.15067 → 0.15076 em 1,5s), a barra do herói em
36%, o botão "ver desde o início" aparecendo ao focar a Tela Quente e sumindo
nos canais da casa, e sintonizar o Odeon 1 abrindo o player.

### Limpeza junto

O bloco CSS da grade e dos cartões da R6/R6b saiu — 253 linhas cujos
componentes (`Grade`, `CartaoCanal`) foram substituídos. Não era só higiene:
`.grade`, `.canal` e `.agulha` colidiam com os nomes novos.

### Adendo à R13: a virada de programa, e o intervalo que faltava

O canal da casa não emendava: o filme acabava e o player ficava lá. A grade já
sabia qual era o próximo — faltava o player olhar o relógio.

**Quem manda é o relógio, não o `ended` do vídeo.** A emissora troca de
programa no horário, e o arquivo pode acabar antes (arquivo mais curto que o
`runtime` do provider) ou depois (créditos). O `ended` entra só como sinal de
"acabou cedo".

**E acabar cedo NÃO adianta o canal.** Emendar no próximo faria o canal correr
na frente da própria grade — e a grade é o que todo mundo está vendo: quem
sintonizasse depois cairia noutro ponto do filme. Uma emissora com tempo
sobrando entra em **intervalo** e espera o horário. É o que este faz.

**O buraco que o teste achou: os 4 minutos entre programas.** A grade tem
respiro entre um filme e o seguinte, e nesse vão `emCartaz` devolve `null` —
não há nada no ar. A primeira versão só agendava o **fim** do programa; no
instante seguinte já não havia bloco, o `sintonizar` saía calado, e o canal
ficava parado no filme que tinha acabado.

Agora o player agenda os dois eventos: o fim (→ intervalo) e a estreia do
próximo (→ sintoniza). O herói mostra `INTERVALO` com o que vem e a que horas,
e sintonizar durante o vão responde *"intervalo — Joias Brutas começa às
14:32"* em vez de não fazer nada.

**Um estado que grudou.** O "acabou cedo" era um booleano, e o cartão de
intervalo continuava aberto por cima do programa seguinte. Virou o **id do
bloco** que acabou cedo: quando a programação vira, `noAr` passa a ser outro
bloco, a comparação deixa de bater e o estado se limpa sozinho.

E a sessão antiga é encerrada na virada. Sem isso, um canal que roda a noite
toda deixaria um ffmpeg vivo por filme até o ceifador dos 90s notar.

### Como se testa uma virada que só acontece daqui a uma hora

Relógio deslocado: `Date.now` é substituído por `real() + delta`, com o delta
escolhido para que "agora" caia poucos segundos antes da fronteira. O tempo
continua andando normalmente — só o ponto de partida muda.

**E o script tem que ser injetado na página.** As duas primeiras tentativas
deram falso negativo porque o `Date.now` substituído era o do **sandbox do
Marionette**, que é um realm diferente do da página: o `document` é
compartilhado, o `Date` não. Um `<script>` acrescentado ao documento roda no
realm certo.

Dois cuidados que o teste ensinou:

- **O título não serve de sinal.** No intervalo o herói já anuncia o que vem,
  então ele não muda quando o programa estreia. Quem muda é o selo
  (`INTERVALO` → `NO AR`).
- **Pular o relógio não adianta `setTimeout` já armado**, que corre em tempo
  real. Para observar a estreia, o relógio tem que parar *dentro* do intervalo,
  a segundos dela — não antes do fim do programa anterior.

Verificado ponta a ponta: fim do programa → intervalo anunciando "Joias
Brutas" às 14:32 → estreia sozinha, selo virando `NO AR · Odeon Corujão ·
Thriller`. E o player já tinha sido observado passando de "Corra!" para "Joias
Brutas" por conta própria.

## 26. R15 — para você, com estados

A tela apresentava um ranking sobre um perfil vazio. Medido antes de mexer:

- **87 eventos, 12 obras, um dia. 0 obras terminadas, 0 ♥/✕.**
- **12 dos 24 itens não tinham identificação nem capa** — e entre eles obras
  `ignored`, que a biblioteca esconde desde a R3 e o recomendador oferecia.
  Eram 1.234 delas elegíveis.
- Os postos 2 a 7 tinham scores 0,4257 → 0,4176: **0,8% de diferença
  apresentada como ordem**, numerada de 2 a 7.
- Duas frases preenchiam 22 das justificativas: *"você costuma terminar série
  (100%)"* 11× e *"da sua biblioteca, ainda não assistida"* 11×.

Ou seja: a tela não estava crua. Estava **confiante sobre o que não sabia**.

Uma ideia que a medição matou antes de virar código: uma faixa "chegou agora".
As 8.608 obras identificadas entraram na **mesma varredura** — não há "recente"
neste acervo ainda.

### Consertar a entrada antes da tela

`CANDIDATES_BODY` passou a exigir `match_state IN ('auto','confirmed')` e
`artwork ? 'poster'`. Sobram 8.596 obras, que é acervo de sobra. Não é escolha
editorial: recomendar exige saber o que se está recomendando, e material sem
identificação se resolve na fila de revisão, que é onde ele mora.

Resultado: de 12 itens ruins em 24 para **0 em 20**.

### A tela ganha estados

`conhecimento` = `terminadas × 2 + curtidas + bloqueadas`, normalizado por 6.
Terminar vale o dobro de votar — é a hierarquia do M5, agora exposta em vez de
implícita.

Abaixo de 1, a tela **admite**: cabeçalho dizendo que não te conhece,
termômetro, "continue de onde parou" (o único sinal forte que existe hoje), e a
calibragem. Acima, o herói da marquise volta.

### O motivo virou cabeçalho de seção

Uma justificativa só significa alguma coisa quando **distingue um grupo do
outro**. Embaixo de seis cartões idênticos ela era ruído. E o ranking falso
saiu junto: numerar itens separados por 0,8% é afirmar precisão que o score não
tem.

### A calibragem, e por que ela existe

`GET /api/curation/calibrar`: seis capas, uma por gênero, nunca votadas nem
assistidas, determinísticas no dia (`md5(dia || id)`) — uma fileira que se
rearranja a cada recarga faz a pessoa parar de votar no que sumiu.

O ♥/✕ é o **único sinal legível antes de alguém terminar alguma coisa**. Com 0
terminadas e 0 votos, sem isso a tela abre pedindo desculpa e não oferece
saída.

### As lâmpadas finalmente acendem

A moldura de bulbos do herói existe desde a R1 e **desde a R1 estava apagada**.

O truque foi não animar bulbo a bulbo: os pontos são um gradiente repetido, não
elementos. Uma segunda camada idêntica, recortada por uma máscara em faixa que
desliza, acende só os bulbos que a faixa cobre. Uma propriedade animada, nenhum
nó a mais — e o herói, o player e o ao vivo ganharam juntos, porque os três já
usavam a mesma classe.

O contador da calibragem é a mesma linguagem: não é "2 de 6", são **seis
lâmpadas**, e votar acende uma. O quanto ele te conhece é literalmente o quanto
a marquise está acesa. E o ✕ tomba a capa pra fora do quadro — antes ela sumia,
e sumia sem deixar claro que foi você que tirou.

### Duas armadilhas de flexbox, a mesma raiz

As capas da calibragem saíam serrilhadas, e por dois motivos em sequência:

1. Como item de um container flex em coluna, a altura do `.pv-arte` vinha do
   **conteúdo** — a imagem no tamanho natural — e o `aspect-ratio` era
   ignorado. Virou bloco.
2. Corrigida a proporção, as **larguras** ainda variavam (191, 185, 138…). O
   mínimo de um item flex é `auto`, que vale o min-content — e com o título em
   `nowrap`, isso é a largura do texto inteiro. `min-width: 0` resolve. É a
   mesma armadilha do §15, na mesma família de bug.

Verificado no app: seis capas em 138×207, todas iguais; o voto acendendo uma
lâmpada e o contador indo de 0/6 a 1/6; duas obras em "continue de onde parou";
e **zero lixo na tela**.

## 27. R16 — a área de administração

O Odeon já sabia fazer tudo o que esta tela faz. **Sete rotas existiam sem
nenhum cliente**: listar/criar/alterar/apagar usuários, listar sessões,
histórico de trabalhos e as quatro manutenções. Quatro delas só eram
alcançáveis por `curl` — o que significa, na prática, que só eu conseguia
usá-las, e só com o terminal aberto. A R16 não é funcionalidade nova; é a tela
que faltava para um poder que já estava lá.

### Aba própria, não um canto do "para você"

A alternativa era pendurar administração na tela de entrada. Não: o "para você"
tem uma tese ("uma biblioteca que te conhece") e manutenção dentro dela é
exatamente o erro que a R1 corrigiu ao tirar `varrer`/`sprites`/`identificar` da
topbar. A aba `admin` só aparece para quem é admin, e some para todo mundo mais.

### O ensaio vem antes

As quatro manutenções escrevem em disco ou chamam o provider. Nenhuma roda
direto: o botão é **ensaiar**, que conta o que faria e não escreve nada, e só
depois — com o número na frente — aparece o `executar`. "Apaga imagem em disco
que nenhuma linha do banco referencia" é uma frase confortável até a contagem
vir 12.000. O número antes da ação é o que transforma um botão perigoso em uma
decisão.

As quatro rotas devolvem **quatro formatos diferentes** de resposta (nenhuma
foi escrita pensando em ter cliente). Em vez de uniformizar quatro handlers e
arriscar o que já funciona, a tela tem um `descreve()` que normaliza os quatro —
a bagunça fica num lugar só, e visível.

### Os cadeados moram no backend

A tela não oferece "rebaixar" na própria conta e não oferece "remover" em quem
está sozinho como admin. Isso é cortesia de interface, não segurança: quem
recusa é o `update_user`, que devolve *"não dá pra tirar o próprio acesso"* e
*"esse é o último admin"*. Um cadeado que só existe no React é um cadeado que
não existe.

### A sessão precisou de nome próprio

Encerrar **um** aparelho exige nomear a sessão, e `auth_session` não tinha `id`:
a chave primária é o `token_hash`. Eu assumi que existia e a rota 500 na
primeira chamada. Mandar o hash do token até o navegador só para virar o `key`
de uma linha de tabela é higiene ruim — é material derivado do segredo de
autenticação. Daí a migração `0016`, que adiciona `id uuid` com
`DEFAULT gen_random_uuid()`: as linhas existentes ganham identificador sozinhas,
então **ninguém é deslogado** pela migração.

Verificado ponta a ponta: sessão descartável → `/api/auth/me` 200 → clique em
`encerrar` na tela → o mesmo token passa a dar 401, e a sessão de quem clicou
continua viva.

### Duas armadilhas

1. **Migração nova não aplica só com `restart`.** O `sqlx::migrate!` embute o
   diretório no binário em tempo de compilação, e o `cargo watch` observa
   `src/`, não `migrations/`. Um `.sql` novo sozinho nunca dispara recompilação:
   o container reinicia o binário velho e a migração some sem erro nenhum.
   Tocar num `.rs` resolve.
2. **`.num` já existia.** A tabela usou `class="num"` para as colunas de data, e
   `.num` é a pílula com fundo e borda do catálogo — cada data ganhou uma caixa
   em volta. Nome genérico numa folha de estilo global é armadilha; virou
   `.adm-num`.

## 28. R17 — a foto que já estava lá

O pedido foi "alguns programas do ao vivo não têm foto de fundo, como *Uma
Família da Pesada*; nesse caso use outra foto". A resposta certa acabou não
sendo usar outra foto: **aquela foto existia desde sempre, e o Odeon a jogava
fora.**

### O que estava acontecendo

Um programa da grade só ganhava capa se o título casasse com uma obra da
biblioteca — a regra conservadora do §17, que se recusa a chutar entre 734
títulos ambíguos. Ela funciona, e é pouca: 233 de 926 programas, 25%. *Uma
Família da Pesada* não está na biblioteca, então ficava com fundo preto.

Só que o XMLTV do ErsatzTV manda a imagem junto, e em três sabores:

| o que vem | quantos programas | serve de fundo? |
|---|---|---|
| `<image type="still" orient="L">` | 273 | é o ideal: quadro do episódio, deitado |
| `<image type="poster">` | 162 | serve, cortado |
| só `<icon src="…">` | 392 | serve, cortado |
| nada | 92 | — |

827 de 919. O importador lia título, subtítulo, descrição, ano e categoria, e
passava direto pelas imagens.

Depois de importar: **90% dos programas com foto**, contra 25%.

### Três decisões no caminho

**Baixar, não apontar.** A URL que o ErsatzTV publica é um endereço da bridge
do Docker — existe só nesta máquina. Apontar o navegador pra lá funcionaria
aqui e em nenhum outro aparelho da tailnet, e deixaria a grade dependendo do
ErsatzTV estar de pé pra ter capa. A imagem passa a ser do Odeon, como o resto
do artwork.

**Pedir maior.** O XMLTV pede `fillHeight=220` ou `440` — bom pro cartão de um
guia, pobre pra um fundo de 46vh. O servidor de imagem aceita o pedido maior, e
a imagem chega em 1280×720. A troca é conservadora: só mexe se o parâmetro já
estiver lá e pedir menos que 720; URL de provedor desconhecido passa intacta.

**Não rebaixar a cada 6 horas.** A grade é substituída inteira a cada
importação, e o vigia do §24 importa sozinho. Sem cache seriam centenas de
downloads a cada ciclo, pra sempre. O nome do arquivo sai de um hash da URL, e
o que já está em disco é reaproveitado. É FNV-1a e não o `DefaultHasher` da
std: aquele é semeado por processo, e daria um nome novo a cada reinício do
servidor — o cache nunca acertaria.

### A ordem de preferência tem motivo

`COALESCE(w.artwork->>'backdrop', p.arte, w.artwork->>'poster')`

O `backdrop` da obra é deitado e veio do provedor de metadados: melhor fundo não
existe. Depois vem a arte do **programa**, que costuma ser um quadro daquele
episódio — mais específica que o pôster da série, que serve pra temporada
inteira. O pôster fica por último porque é em pé e vai ser cortado.

### E os que realmente não têm foto nenhuma

91 dos 92 são o mesmo canal: o *Clipe Show*, de clipes musicais, que o XMLTV não
ilustra. Aí não há foto a inventar, e a escolhida foi **a marquise da casa**: o
fundo âmbar com a moldura e a fileira de lâmpadas correndo — a mesma `.bulbs`
do herói e do player, sem efeito novo. Quando o Odeon não tem o que mostrar, ele
mostra a si mesmo. A alternativa considerada era uma cor derivada do título;
dá variedade, mas é cor inventada, e não diz nada sobre a obra.

Duas correções que só apareceram porque este canal virou visível:

- As lâmpadas coladas no topo do bloco liam como **borda**, não como marquise.
  Desceram pra cima da moldura, que é onde uma marquise tem lâmpada — e a
  opacidade subiu de 0.42 pra 0.85, porque sobre foto elas são enfeite e aqui
  são a única coisa que existe pra ver.
- Nem todo provedor manda título de gente: o Clipe Show manda o nome do
  arquivo. `Bo.Burnham.Inside.2021.1080p.WEBRip…` é **uma palavra** pro
  navegador, e saía pela direita do bloco. `overflow-wrap: anywhere`.

### A armadilha que quase apagou tudo

`programme.arte` teve que entrar na consulta de "artwork vivo" da limpeza de
órfãos (§27). Sem essa linha, a manutenção *Limpar artwork órfão* apagaria a
foto de **todos** os programas no ar — nenhum deles está em `work` nem em
`collection`, e a limpeza os leria como lixo. A consulta estava duplicada nos
dois lados da limpeza (ensaio e execução); virou uma constante só, porque duas
cópias divergindo significaria contar uma coisa e apagar outra.
