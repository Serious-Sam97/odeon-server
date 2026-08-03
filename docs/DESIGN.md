# Odeon — decisões de arquitetura

Documento vivo. Registra **por que** cada escolha foi feita, pra que daqui a seis
meses ninguém (inclusive eu) desfaça uma decisão boa por esquecer o motivo.

> **Aviso de leitura, 03/08/2026.** As seções **§30 e §35 a §42** foram escritas
> em cima de um documento de ideias que se revelou uma *interpretação* da visão
> de quem decide, não a visão. Elas ficam aqui inteiras — o raciocínio e as
> medições continuam válidos —, mas **várias das decisões que elas registram
> foram revistas**. Cada uma dessas seções começa com um bloco de revisão
> dizendo o quê e apontando para o `IDEIAS.md`, que foi reescrito a partir das
> anotações originais.
>
> A lição, que vale mais que qualquer uma delas: **prosa convincente sobre uma
> decisão errada é mais difícil de desfazer do que a decisão.**
>
> A **§44** é a primeira seção escrita depois do realinhamento, e é a que desfaz
> o maior dos desvios: o círculo.

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

## 29. O título que o disco estragou

O pedido foi "arruma o título do Clipe Show". O canal mostrava
`Bo.Burnham.Inside.2021.1080p.WEBRip.x264.AAC5.1-[YTS.MX]` e
`Happy Tree Friends： Still Alive`. Medindo antes de mexer, o canal virou o
sintoma de uma coisa muito maior: **1.542 obras da biblioteca — quase 9% —
têm o mesmo defeito.**

### O sósia

Nenhum sistema de arquivos aceita `/ \ : * ? " < >` num nome, e todo downloader
resolve isso do mesmo jeito: troca o caractere por um **sósia Unicode** que
parece com ele e não é. `：` (U+FF1A) no lugar do dois-pontos, `？` no lugar da
interrogação, `⧸` no lugar da barra. O arquivo sobrevive no disco; o título
chega ao Odeon escrito errado.

Neste acervo:

| sósia | o que era | obras |
|---|---|---|
| `：` | `:` | 1.246 |
| `？` | `?` | 529 |
| `｜` | `\|` | 145 |
| `⧸` | `/` | 109 |
| `＂` | `"` | 64 |

O `：` sozinho aparece em 1.246 — é o dois-pontos de subtítulo, que quase toda
série tem. `AC⧸DC`, `Snow What？ That's What`, `Salad Fingers 10： Birthday`.

O conserto mora no `scanner::guess`, que é o funil por onde **todo** nome de
arquivo e de pasta passa. Desfazer é seguro exatamente ali porque a entrada é,
por definição, um nome de arquivo: se o sósia está no nome, foi o downloader que
o pôs. Um título que não tem sósia nenhum sai na primeira linha da função, sem
pagar nada.

E como o conserto é no parser, a manutenção **Reprocessar o parse** (§27) já
sabia aplicá-lo ao que estava gravado: 1.540 obras corrigidas de uma vez. Só o
ensaio, antes, mostrando o "de → para" — que é a razão de o botão ser esse.

O ensaio revelou 212 mudanças **além** das do sósia: `Episódio 61` →
`O Pequeno Urso`, com o número do episódio preservado. Não são desta correção —
são melhorias de parser de sessões anteriores que nunca tinham sido aplicadas
aos dados. O reparse só toca obras `unmatched`/`needs_review`, então nada
casado com o provider foi tocado. Ficaram 2 obras com sósia, as duas
`ignored` — que é o certo: `ignored` é obra que alguém descartou de propósito.

### Do lado do ao vivo

O ErsatzTV manda o que a biblioteca dele tem, e a biblioteca dele tem nome de
arquivo. Duas limpezas no importador, e a segunda é conservadora de propósito:
o parser de nome de arquivo só entra quando o título **não tem espaço nenhum e
tem ponto**, que é a assinatura de um release. `Bee Gees - One Night Only - 1997
(Full Concert HD)` é título de verdade e passa intacto — mandá-lo pro parser
custaria o `(Full Concert HD)` sem ganhar nada. Resultado: 30 sósias e 15 nomes
de release, todos zerados, sem tocar nos outros 61 títulos do canal.

Os 2 lembretes agendados sobreviveram, o que não era óbvio: a identidade de um
programa entre importações é canal + horário + **título** (§25), e mudar o
título mudaria a identidade. Nenhum dos dois estava num programa renomeado.

### O que só aparece quando o título fica legível

Corrigido o nome, o canal passou a mostrar títulos de vídeo do YouTube — e
`Cyberpunk: Edgerunners | "I Really Want to Stay At Your House" by Rosa Walton
| Music Video` toma **quatro linhas** e empurra a barra de progresso e o
`SINTONIZAR` pra fora do bloco, que tem altura fixa. Três linhas e reticências.

É o par exato do `overflow-wrap: anywhere` que o mesmo canal já tinha exigido:
lá o problema era uma palavra longa demais pra caber na largura, aqui são
palavras demais pra caber na altura. Os dois vieram do mesmo lugar.

## 30. R18 — o guia de cinema

> **Feito na R34 (§50).** A revista existe: tema da semana sorteado do acervo
> (cinco eixos, incluindo as sagas que a R32 materializou), ensaio gerado por
> LLM sobre fatos do banco — com selo, e omitido quando não há chave — e um
> evento em cartaz que dá XP e conquista. **O índice desta seção não morreu:**
> ele desceu e virou a parte de consulta, atrás da capa.
>
> **Revisto em 03/08/2026.** O que foi entregue é um **índice** — cartões por
> pessoa, gênero e década, para consulta. A visão pede uma **revista que muda**:
> tema por semana, eventos de um filme ou saga para incentivar a assistir, e o
> acervo servindo para ensinar história do cinema. E **igual para todo mundo**,
> de propósito, para haver assunto em comum.
>
> O índice não morre: vira a camada de consulta atrás da revista.
>
> Ver `IDEIAS.md` §3.1.

Primeira fase saída do `docs/IDEIAS.md`, e a que a medição escolheu: **418
diretores distintos, 134 com duas obras ou mais, e cobertura de direção de
100% nos filmes identificados** (548 de 548). Não faltava dado. Faltava tela.

### A tese, e por que ela exige o histórico

Um guia de cinema que qualquer site tem é a Wikipédia com passos extras. Então
nenhuma página aqui é biografia: toda pessoa responde três coisas ao mesmo
tempo — quem é, **o que disso você tem**, e **o que você fez com isso**. A
terceira é a que só este servidor pode dar, porque é o `credit` (§8h) cruzado
com o histórico que existe desde o M0.

Backend novo: **nenhuma tabela, nenhuma migração.** Duas rotas (`/api/guia` e
`/api/guia/pessoas`) sobre o que já estava lá.

### A contagem estava errada, e o próprio dado denunciou

A primeira versão contava `work`, e o ranking de direção saiu assim:

```
Enrique Segoviano  221 obras   (Chaves e Chapolin)
Joseph Barbera     166 obras   (Tom & Jerry)
```

São episódios. É a falha da R3 (§14) — *"14.657 episódios não são uma
biblioteca"* — reaparecendo num eixo novo. E havia uma pista no próprio
resultado: quem tinha 221 obras tinha **um** pôster distinto, porque desde a R9
(§21) o episódio herda a arte da série.

A unidade passou a ser o **título**, com o mesmo rollup de `/api/library`, e
vale a regra que a locadora já usava: uma série é uma caixa, não vinte e uma
(§20). Depois disso o eixo virou o que devia ser:

```
Chris Columbus 7 · Robert Zemeckis 7 · James Cameron 6
Hans Zimmer 16 · Alan Silvestri 15 · John Williams 13
```

**Um resíduo medido e aceito:** 92 episódios identificados (1,1% de 8.068) não
estão em nenhuma coleção `season`/`series`, e cada um conta como título
próprio — é o que infla o elenco de *Arrested Development* para 26. Isso é
qualidade de dado, não do guia, e consertar num caminho de leitura seria
maquiar. É material para o painel de saúde do §24, que existe justamente pra
mostrar o que está torto.

### O bug do agrupamento: pôster nunca é chave

Depois do rollup, um diretor de 43 episódios de UMA série continuava voltando
como 43 títulos. A causa era o `GROUP BY` incluir o pôster junto do id do
grupo: quando a série não tem arte própria, cada episódio cai no pôster dele
mesmo e o grupo se parte. Pôster é **agregado** (`max`), nunca chave.

O caso existe neste acervo porque a única série de AniList é justamente a que
o reparo da §21 não conseguiu enriquecer.

### Duas telas discordando sobre a mesma palavra

`terminadas` foi escrito lendo `playback_state.finished` — que é **falso nas 16
linhas** deste acervo. Ao lado, a tela "para você" anunciava *2 obras
terminadas*.

Quem está certo é o M5: terminada é `event_type = 'finish'` **ou** ter passado
de 92% da duração (§8f). Não existe um único evento `finish` neste acervo, então
os dois casos vêm todos da razão — que é exatamente por que aquela regra tem as
duas metades. O guia passou a ler a mesma fonte, e o resultado é verificável:
**Martin Campbell, 1 terminado de 2** — ele dirigiu *Cassino Royale*.

### E o defeito que isso desenterrou

`playback_state.finished` não é apenas nulo: ele é **apagado**. O `ON CONFLICT`
do `POST /api/works/{id}/progress` faz `finished = EXCLUDED.finished`, então
reabrir no minuto 30 um filme já terminado desmarca o "terminado".

A prova está na própria linha: *Cassino Royale* tem `play_count = 1` — e o
contador só incrementa na transição falso→verdadeiro — com `finished = false`.

Isso é anterior à R18 e afeta os contadores de "vistos" da biblioteca e da
temporada. A curadoria escapa por ler `play_event`. Corrigido logo depois, a
pedido de quem administra — ver §31.

### O botão que mentia

O mínimo de obras por pessoa estava escrito em dois lugares — 3 na capa, 2 na
lista completa. O efeito: o botão dizia *"ver as 644"* e abria uma lista de
**1.424**. Um total que muda ao atravessar o próprio botão é pior que não ter
total, e é a mesma família do "Biblioteca 300" que a R3 (§14) corrigiu.

Agora o número mora em `minimo_de(role)` e as duas rotas o consultam. Elenco
pede 3 porque é o eixo com mais gente por natureza (73.507 créditos contra
1.191 de direção); o resto fica em 2.

### Um número certo pode fazer a tela parecer quebrada

O cartão dizia "7 títulos" e a ficha listava 12 obras logo abaixo. Os dois
estavam certos: o guia conta **um papel** (direção) e a filmografia traz todos
os créditos — Columbus também assina roteiro e produção. Dois acertos que juntos
lêem como defeito.

A ficha passou a dizer de que papel é a contagem: *"7 títulos em direção · 12
obras no acervo contando os outros papéis"*.

### O que não entrou, e por quê

- **Produção**, apesar de ser o segundo maior volume (45.741 créditos contra
  1.191 de direção). A allowlist do §8h já tinha decidido isso uma vez: um eixo
  de produção enterraria a direção em assistente de efeitos.
- **Região.** Conferido em `metadata/tmdb.rs`: o Odeon busca título, sinopse,
  ano, gêneros e arte — país, idioma, empresa e orçamento não têm coluna. É
  `ALTER TABLE` mais uma revisita de 548 filmes, e está planejado como R22.
- **Selo de "visto" no cartão da filmografia.** O cabeçalho fala de histórico
  (o máximo que você já alcançou); o cartão só recebe a posição atual. São
  perguntas diferentes, e um selo ali alegaria o que o payload não sabe. Ficou
  a barra de progresso, como no resto do app.

### Gênero e década contam filmes, de propósito

Num acervo com 14.657 episódios contra 635 filmes, contar tudo faria "Drama"
significar "uma série longa que eu tenho". São 19 gêneros e 7 décadas, e os dois
eixos não ganharam tela própria: caem no filtro de `/api/works`, que resolve tag
e faixa de ano desde o M2.

### Verificação

Contra o acervo real, com Firefox por Marionette (o método do §23, reescrito —
o `scratchpad/` original não está no repositório):

| | |
|---|---|
| `/api/guia` | 200 em 0,59s, 18 KB |
| `/api/guia/pessoas` | 0,07s (direção) · 0,38s (elenco, 73.507 créditos) · 0,08s (busca) |
| a capa | 5 seções, 36 pessoas, 26 faixas, sem estouro horizontal |
| busca "Campbell" | *"Martin Campbell · 2 títulos · 1 terminado"* |
| a ficha | *"2 títulos em direção"*, *"você terminou 1"*, os dois Bond com a barra de progresso |
| a 700px | nada estoura na horizontal |

A linha do histórico só aparece em quem tem histórico — **2 diretores, 10 do
elenco, 3 da trilha**. É a regra do §24 aplicada ao cartão: linha limpa some.
Escrever "0 terminadas" em 127 cartões ensina a não ler o cartão, e aí o número
que importar também não será lido.

## 31. `finished` é acumulativo, não o estado do instante

O defeito que a R18 desenterrou (§30), consertado.

### O que estava errado

O upsert de `POST /api/works/{id}/progress` fazia `finished = EXCLUDED.finished`.
Isso transforma o campo na resposta a *"você está no fim agora?"*, quando a
pergunta que quatro telas fazem é *"você já terminou isto alguma vez?"*. Reabrir
no minuto 30 um filme já visto apagava o visto.

A prova estava na própria linha, e é o tipo de evidência que só aparece olhando
dado real: 16 linhas em `playback_state`, **zero com `finished`**, e ainda assim
*007: Cassino Royale* com `play_count = 1` — contador que só sobe na transição
falso→verdadeiro. Era o fóssil de um `finished` que existiu e foi sobrescrito.

### O conserto foi de duas linhas, não de uma

A primeira é óbvia: `finished = playback_state.finished OR EXCLUDED.finished`.

A segunda não era, e teria passado despercebida: o `play_count` incrementava
com `WHEN EXCLUDED.finished AND NOT playback_state.finished`. Com o `finished`
grudando, essa condição nunca mais seria verdadeira — o contador congelaria em 1
para sempre, **levando junto o bônus de reassistir do M5** (§8f), que é o sinal
positivo mais forte que o perfil de gosto tem. Consertar metade teria trocado um
bug visível por um invisível.

Agora quem decide o incremento é a **posição guardada**: conta como exibição
nova quem chega ao fim vindo de um ponto que ainda não estava no fim. Isso
resolve o congelamento e, de brinde, o outro erro que a condição antiga
escondia — sem ele, cada heartbeat depois dos 92% somaria um.

### O passado era recuperável, e por um motivo desenhado no M0

`playback_state` é, nas palavras do §8, *"só um cache derivado"* — e o
`play_event` **nunca é sobrescrito**. Então a migração `0018` reconstrói o campo
a partir do log, com a regra do §8f (evento `finish` **ou** mais de 92%), que é
a mesma que a curadoria e o guia usam. A decisão de guardar o log cru desde o M0
paga aqui pela terceira vez.

A migração **só liga** o `finished`, nunca desliga. Se alguma linha estiver
marcada sem que o log comprove, quem está incompleto é o log — eventos podem
ter se perdido, e apagar a marca destruiria informação que não volta.

### Verificação

O upsert foi exercitado com o SQL exato do handler, contra a linha real, **dentro
de uma transação desfeita no fim** — testar exclusão e sobrescrita no acervo de
alguém não é opção (é a mesma postura da §22):

| passo | esperado | resultado |
|---|---|---|
| reabriu no minuto 30 | `finished` continua `true`, `play_count` 1 | ✅ |
| reassistiu até o fim | `play_count` 1 → 2 | ✅ |
| heartbeat ainda no fim | `play_count` **continua** 2 | ✅ |
| depois do `ROLLBACK` | linha idêntica à de antes | ✅ |

E o efeito nas telas, depois da migração: `/api/library` voltou a reportar
`finished_count` (duas entradas — *Cassino Royale* e *As Visões da Raven*), e o
selo "visto" voltou ao cartão da filmografia no guia. Ele convive com a barra de
progresso e os dois dizem coisas diferentes de propósito: o selo é histórico
("já terminei alguma vez"), a barra é o agora ("estou em 20% de uma revisão").

### A armadilha do §11, pela terceira vez

A migração não aplicou na primeira tentativa. `sqlx::migrate!` embute o
diretório em tempo de compilação e o `cargo watch` observa `src/`, não
`migrations/` — o binário tinha recompilado por causa do `works.rs` **antes** de
o `.sql` existir, e subiu dizendo "migrations em dia" sem aplicar nada. Um
`touch` em qualquer `.rs` resolve.

Está registrado no §11 e na R16, e mordeu de novo assim mesmo. Fica a nota
prática: ao acrescentar migração, o `.rs` tem que ser tocado **depois** do
`.sql`, não antes.

## 32. Curiosidades — e de onde elas podem vir

> **Revisto em 03/08/2026, parcialmente.** As curiosidades entregues aqui e no
> §33 foram **aprovadas e ficam como estão**.
>
> O que mudou é a regra: a recusa de LLM registrada nesta seção continua
> valendo para **fato sobre filme**, mas **foi levantada para conteúdo
> editorial** — o guia dinâmico e os eventos temáticos podem ser escritos por
> LLM, com os fatos vindo do banco e o texto marcado como gerado.
>
> Ver `IDEIAS.md` §2.3.

Pedido: "curiosidades sobre o filme para a pessoa aprender/se entreter", dentro
da ficha. A parte difícil não foi a tela — foi a **fonte**.

### Três fontes descartadas, e a regra que as descartou

| fonte | por que não |
|---|---|
| TMDB / AniList | **não têm trivia.** O que devolvem é ficha: título, sinopse, ano, gênero, elenco |
| Wikipédia / Wikidata | tem, mas é texto solto, com licença e uma dependência de rede nova por obra |
| **um LLM gerando** | inventa com confiança — e é exatamente o que o §18 proíbe |

A terceira é a tentação de 2026 e é a pior. O §18 já fixou a regra ao recusar
chutar "inglês" para uma legenda sem sufixo: **não se inventa metadado**. Uma
curiosidade falsa sobre um filme que a pessoa ama é pior que nenhuma, e ela
chega com a mesma cara de verdade que as outras.

### A fonte que sobrou é a melhor: o próprio grafo

Curiosidade sobre o filme qualquer site tem. Curiosidade sobre **o seu acervo** e
**o seu histórico com aquele filme** só este servidor pode dar — é o argumento da
R18 (§30) uma camada abaixo, e sai de graça de `credit`, `work_tag`,
`media_file` e `playback_state`.

Sete consultas, cada uma com um limiar. Medido em *007: Cassino Royale*:

```
✦ De Martin Campbell você também tem 007 Contra GoldenEye.
♪ A trilha é de David Arnold, que assina outras 19 obras do seu acervo.
◎ Daniel Craig e Jeffrey Wright também dividem a tela em 007: Quantum of Solace (2008).
● Você já viu este filme inteiro.
```

O reencontro de elenco é a que mais parece curiosidade — e ela é **impossível
sem a deduplicação por `provider_key` do §8h**: sem aquilo, "Daniel Craig" seria
uma linha por filme e nunca cruzaria com nada.

### Só nasce se for notável

Uma curiosidade que vale pra toda obra não é curiosidade. Então cada consulta
tem um limiar, e quando ele não é atingido a linha **não existe** — não vira
"informação indisponível". É o §24 aplicado a entretenimento.

Na mesma ficha acima, duas não dispararam e é o comportamento certo: gênero
(Ação tem 200 filmes no acervo — não é raridade) e duração (145 min, com 56
filmes mais longos). A seção inteira some quando a obra não rende nada.

### O defeito que a verificação achou: "você também tem 1408"

Na ficha de *1408*, a rota dizia *"De Mikael Håfström você também tem 1408"* e
*"John Cusack e Mary McCormack também dividem a tela em 1408"*.

Estava tecnicamente certa: são **duas linhas em `work`** com o mesmo
`{"tmdb": "3021"}`, uma `auto` e outra `confirmed` — o mesmo filme, dois
arquivos. O acervo tem **6 grupos assim**.

`media_file.path` é UNIQUE, mas nada impede duas obras com o mesmo título — e o
§8b até favorece isso, porque o matcher nunca sobrescreve o que um humano
confirmou. A comparação certa não é `w2.id <> w.id`, é pelo **id do provider**,
que é quem responde "é o mesmo filme?". Virou a constante `OUTRA_OBRA`, usada
pelas três consultas que dizem "você também tem".

### Detalhes de apresentação

- **A frase é montada no servidor**, como os `reasons` do score do §8b. Montá-la
  no cliente seria uma segunda gramática pra manter.
- **Buscada depois do cartaz**, em rota própria: são sete consultas e a sinopse
  não espera por elas. Enquanto não chegam não há esqueleto nem "carregando" —
  um bloco cinza piscando por 200ms no meio da leitura é pior que a seção
  aparecer quando estiver pronta.
- **Símbolos de texto, não emoji.** `✦ ♪ ◎ ●` e não 🎬 — o Odeon é preto, âmbar e
  a cor da obra, e emoji colorido rompe isso. Mesma razão pela qual a marca é `◉`.
- **A linha sobre você é a única em âmbar**, e vem por último: a leitura começa
  na obra e termina em quem está lendo.

### A aba virou "wiki"

Mudança de nome pedida, e ela é só a etiqueta: o componente continua `Guia.tsx`
e as rotas continuam `/api/guia`. Renomear o interno seria churn sem ganho —
mas fica registrado aqui pra ninguém procurar um `Wiki.tsx` que não existe.

### Verificação

Contra o acervo real, com Firefox por Marionette:

| | |
|---|---|
| `/api/works/{id}/curiosidades` | 0,19–0,23 s em quatro filmes |
| *Cassino Royale* | 4 curiosidades, incluindo a de reencontro |
| *Corra!* | *"Bradley Whitford e Stephen Root também dividem a tela em O Homem Bicentenário (1999)"* |
| *1408*, depois do conserto | não fala mais de si mesmo |
| *Drive* | uma só — *"você parou faltando 39 minutos"* — e é o esperado |
| a seção na tela | entre a sinopse e o elenco, com a linha de "você" em âmbar |

## 33. A curiosidade sobre o FILME — Wikidata e Wikipédia

Correção de rumo, e a mais direta até aqui. O §32 entregou curiosidades tiradas
do próprio acervo ("de Martin Campbell você também tem…") e elas são boas — mas
não são o que alguém quer dizer com *curiosidade sobre o filme*. Aquilo fala da
sua estante; o pedido era falar da obra.

### A fonte que eu tinha descartado rápido demais

O §32 descartou três fontes e ficou com o grafo. Revisitando, uma quarta não
tinha sido considerada, e é a resposta:

| fonte | veredito |
|---|---|
| TMDB / AniList | continua sem trivia — devolvem ficha |
| **Wikidata** | **é a resposta**: estruturado, CC0, casa pelo id do TMDB |
| Wikipédia | prosa de verdade, CC BY-SA — exige crédito e link |
| um LLM gerando | continua fora, e o §18 continua sendo a razão |

**Por que o Wikidata resolve o que a Wikipédia sozinha não resolvia.** A
propriedade `P4947` é o id do filme no TMDB — o mesmo que o Odeon guarda em
`work.external_ids` desde o M1. O casamento é **exato**: nada de buscar por
título e desempatar por ano. É o princípio do `provider_key` do §8h, e é o que
separa isto do casamento conservador da grade ao vivo (§17), que precisa recusar
734 títulos ambíguos justamente por não ter um id.

Medido em 12 filmes sorteados do acervo: **12 de 12 casaram.** E o mesmo
Wikidata devolve o link do artigo da Wikipédia por sitelink, então nem o título
do artigo é adivinhado.

### O resultado, em dois filmes reais

```
Pulp Fiction
  ★ Ganhou o BAFTA de melhor roteiro original — e mais 22 prêmios.
  $ Custou US$ 8 milhões e arrecadou US$ 108 milhões — 13 vezes o que custou.
  ⌖ Foi filmado em Los Angeles, Tennessee.
  ❝ A filmagem começou em 20 de setembro de 1993, tendo como locações
    diversos pontos de Los Angeles e arredores.            [Wikipédia]

007: Cassino Royale
  ❝ A EON Productions conseguiu os direitos para Casino Royale em 1999,
    depois da Sony Pictures tê-los trocado com a Metro-Goldwyn-Mayer
    pelos direitos de Spider-Man.                          [Wikipédia]
```

A última linha é o tipo de coisa que o pedido descrevia, e ela vem da seção
"Produção" do artigo — cortada **em fim de frase**, nunca por contagem de
caracteres: texto truncado no meio de uma palavra lê como defeito de
renderização, que é a mesma correção da §21 no verso da caixa.

### Cinco defeitos que só a verificação achou

1. **"Ganhou o premiados com o BAFTA de melhor roteiro original."** O rótulo em
   português do Wikidata muitas vezes é o nome de uma **categoria** da
   Wikipédia, não do prêmio. As cascas conhecidas são retiradas.
2. **"national Board of Review"**, com N minúsculo. Eu tinha escrito uma função
   que rebaixava a inicial pra encaixar no meio da frase — e não há como
   distinguir nome próprio de substantivo comum sem saber o que a palavra é. A
   função foi apagada; a capitalização fica como veio.
3. **"É uma adaptação de Drive"**, na ficha de *Drive*. O livro tem o mesmo
   nome, e é isso que a frase deve dizer: *"da obra homônima"*.
4. **Três parágrafos colados num bloco só.** O `extract` da Wikipédia separa
   parágrafos com **um** `\n`, não dois.
5. **O link do Wikidata apontava para uma busca** (`Special:Search?search=680`),
   que procura o texto "680" e não acha filme nenhum. Agora aponta para a
   entidade. Link de conferência que não confere é pior que link nenhum — a
   mesma exigência que o §8b faz do score.

### Duas decisões de honestidade

**Moeda é obrigatória.** `wdt:P2130` devolve o número **sem** a moeda, e
escrever "US$" num orçamento em euros é a mentira com cara de metadado que o
§18 proíbe. A moeda vem pelo caminho completo do statement (`psv:`), e valor em
moeda que não sabemos nomear simplesmente **não vira curiosidade**.

**Prêmio de prestígio é escolhido, o resto é contado.** 23 linhas de prêmio não
é curiosidade, é currículo. Sem uma lista de prestígio, a manchete de *Pulp
Fiction* seria o Dallas-Fort Worth Film Critics Association Award.

### Cache, e por que ele é parte do desenho

`work_trivia` (migração 0019) guarda o resultado por 30 dias. Três razões, e as
três são regras que este projeto já segue:

1. **A ficha não pode depender da rede** — abrir uma obra faria duas chamadas
   externas toda vez;
2. **educação com serviço alheio** — o SPARQL do Wikidata é público e gratuito;
3. **a biblioteca continua funcionando offline**, que é por que artwork e
   retratos moram em disco desde o M1.

Duas sutilezas: **lista vazia é guardada** (senão todo filme sem entrada seria
reconsultado para sempre), mas **falha de rede não é** — gravar o vazio ali
esconderia a trivia por 30 dias por causa de um segundo ruim.

E nada disso derruba a rota: sem rede, as curiosidades do acervo aparecem
sozinhas. Mesma postura do §17 com a arte do programa — o que falta some, o que
existe fica.

### Verificação

| | |
|---|---|
| primeira busca | 0,7 – 1,6 s |
| **segunda (cache)** | **0,25 s** |
| cobertura no Wikidata | 12 de 12 filmes sorteados |
| testes de unidade | 5, cobrindo moeda, corte de frase, casca de prêmio e desambiguador |
| na tela | fatos do filme primeiro, com crédito; o parágrafo da Wikipédia recuado como citação; os do acervo depois |

O que ficou de fora aqui: **aquecer o cache do acervo inteiro**. A trivia chega
quando alguém abre a ficha, e o primeiro a abrir cada filme paga 1,5 s — 548
vezes, uma pessoa de cada vez. Feito logo depois, e não na R22 como esta seção
previa: ver a §34.

## 34. O aquecimento do cache de trivia, e duas lições sobre serviço alheio

A §33 deixou registrado que a trivia chegava quando alguém abria a ficha, e que
aquecer o acervo inteiro ficaria para depois. "Depois" foi agora, e o caminho
até 92,7% de cobertura passou por três defeitos — dois deles da mesma família e
o do meio, o pior, **silencioso**.

### Nasceu como `job`, e isso se pagou no mesmo dia

548 filmes com duas chamadas externas cada não cabem numa requisição HTTP. O §12
já tinha registrado o preço de operações longas viverem na memória do processo, e
o §21 já tinha marcado os reparos síncronos como dívida — então esta rota nasceu
como `job`: estado no banco, progresso visível, cancelamento cooperativo,
retomável pelo `WHERE`.

Não foi cerimônia. Durante a execução o `cargo watch` reiniciou o binário
**quatro vezes** (o `backend/src` estava sendo editado em paralelo), e as quatro
viraram `interrupted` com o progresso preservado. Cada retomada pegou só o que
faltava — 173, depois 123, depois 48, depois 23. Um script síncrono teria
recomeçado do zero quatro vezes, ou pior, teria ficado sem saber o que já fez.

**Custou uma migração:** `job.kind` tem CHECK com a lista de tipos, e `trivia`
não estava nela (0020). O sintoma foi pior que o erro — `Job::start` devolve
`None` tanto para "já existe um ativo" quanto para "o INSERT falhou", então a
rota respondia *"já há um aquecimento em andamento"* quando nunca houvera
nenhum. Erro disfarçado de estado normal, que é o §8b ao contrário. A rota agora
pergunta ao banco antes de afirmar.

### Lição 1: a correção para 429 não é esperar mais, é perguntar menos vezes

A primeira execução consultava **um filme por requisição**, com 250 ms de pausa.
Resultado medido: **513 falhas em 547**, todas iguais —

```
429 Please respect our robots policy and limit your requests to 1 RPS
```

O serviço diz a regra na própria resposta. E a saída óbvia — aumentar a pausa —
seria a pior das duas: 547 requisições continuam sendo 547 requisições.

O SPARQL aceita `VALUES` com uma lista de ids, e o acervo inteiro cabe em **22
consultas**. Ficou mais rápido *e* muito mais educado com um serviço público e
gratuito. Resultado: 513 filmes em menos de um minuto, **zero falhas**.

Junto vieram o recuo exponencial no 429 e um User-Agent que diz o que o programa
é — `Odeon/0.1` sozinho não identifica nada, e a política da Wikimedia pede isso.

### Lição 2: 200 não quer dizer que veio o que se pediu

Com o Wikidata resolvido, o número que não fechava era outro: **14 parágrafos de
produção em 548 filmes**. Baixo demais para ser verdade.

A causa estava na resposta, num campo que eu não lia:

```
"exlimit" was too large for a whole article extracts request, lowered to 1.
```

Extrato de artigo **inteiro** é um por requisição — `exlimit` só passa de 1 para
extratos de introdução, e a introdução não tem seção de produção. Eu pedia 20
títulos, recebia 20 páginas, **e só a primeira trazia texto**. As outras 19
voltavam sem `extract`, com HTTP 200 e sem erro nenhum.

É a §R6d de novo, com outra roupa: *verificar o mecanismo não é verificar o
resultado*. Lá a classe `.idle` entrava e a barra continuava na tela; aqui a
requisição respondia 200 e o texto não vinha.

### E a mesma pedra, na Wikipédia

Corrigido para uma requisição por artigo, o número subiu de 14 para… 14. Porque
500 requisições a 120 ms tropeçaram no **429 da Wikipédia** — e o meu código
tratava erro de rede como "não tem parágrafo", em silêncio.

Duas correções, e a segunda importa mais:

1. um segundo entre artigos, com recuo e retentativa;
2. **falha contada, não engolida.** É o silêncio que faz um defeito destes
   sobreviver: o job terminava anunciando "0 falhas" com quase nada gravado.

### O resultado

| | |
|---|---|
| filmes no cache | **548 de 548** |
| com ao menos uma curiosidade | **508 — 92,7%** |
| curiosidades gravadas | **1.694** |
| fotografia · locação · dinheiro | 433 · 368 · 304 |
| **produção (prosa da Wikipédia)** | **232** — eram 14 |
| adaptação · prêmios | 215 · 142 |

Os 40 filmes sem nada estão gravados como lista vazia de propósito: "procurei e
não há" é resposta, e sem ela cada abertura de ficha reconsultaria o serviço.

### O que fica de dívida

O aquecimento é manual (`POST /api/maintenance/aquecer-trivia`). Filme novo
identificado depois disto busca sob demanda na primeira abertura da ficha, que é
o comportamento da §33 e continua correto — mas o natural é encadear o
aquecimento ao fim da identificação, como `?then=match` faz com a varredura
(§12). Não foi feito aqui porque encadear operação de rede a um job que já é
longo merece a sua própria decisão.

## 35. R19 — o círculo e a fita

> **Revisto em 03/08/2026, em dois pontos, e são os dois maiores desvios da
> série.**
>
> **1. O círculo nunca foi pedido.** Ele foi inventado no documento de ideias
> anterior e virou peça de schema aqui, escopando empréstimo, rotação, notas,
> feed, convite e acesso. A palavra usada por quem decide é **amigos** — que é
> outra coisa: relação entre duas pessoas, cada um com a sua lista, sem grupo.
> A locadora e o estoque são **do servidor**.
>
> **Desfeito na R28 (§44):** o círculo saiu do schema, a escassez virou uma cópia
> por caixa no servidor, e a amizade nasceu no lugar dele.
>
> **Desfeito na R30 (§46), e a recusa era de modelagem.** A fita virou um
> objeto próprio, separado do `playback_state` de qualquer pessoa — então
> rebobinar deixou de apagar o "continuar de onde parou" de alguém e passou a
> mexer só no objeto. Rebobinar a fita de outra pessoa é obrigatório, e ninguém
> perde nada com isso.
>
> **2. A recusa de rebobinar a fita de outra pessoa mata a ideia.** Esta seção
> chama isso de "ação destrutiva entre usuários" e limita o rebobinar à própria
> posição. **O atrito entre as pessoas é justamente a graça**: você põe pra
> tocar, descobre que alguém devolveu no minuto 47, e tem que rebobinar
> esperando alguns segundos. E existe log de quem devolve certo e quem devolve
> zoado.
>
> Ver `IDEIAS.md` §2.1 e §3.9.

A locadora da R8 (§20) era uma **vitrine**: 600 caixas lindas, estado nenhum.
Nada do que se fazia lá deixava marca, e voltar amanhã encontrava exatamente a
mesma loja. Esta fase deu a ela as duas coisas que faltavam pra ser um lugar:
**alguém está com a fita**, e **ela volta em algum estado**.

### O círculo, e por que ele virou peça de schema

"A casa" nunca foi conceito do banco — era `SELECT * FROM app_user`. O círculo
entrou porque resolve três coisas de uma vez, e nenhuma delas é organizacional:

**1. Torna a escassez honesta.** "Este DVD está alugado" é falso quando o
arquivo está sempre lá, e o §18 proíbe dizer coisa falsa com cara de metadado.
Dentro de um círculo deixa de ser falso: a fita **está** com outra pessoa, e é
essa pessoa que está te barrando, não o software. O Odeon não vira DRM de
mentirinha — vira o balcão que informa quem levou.

**2. Dá saída ao bloqueio.** Um bloqueio sem saída é parede. Um que diz *"rudney
está com esta há 3 dias"* tem porta, e é ela que faz a coisa ser um lugar em vez
de um sistema: **pedir de volta**.

**3. Escopa tudo que vem depois.** Empréstimo, rotação (R20), retrospectiva
(R24) e feed (R25) são todos por círculo. Adotá-lo agora custou uma coluna e
evitou uma migração dolorosa — a mesma jogada do `programme.work_id` do §17.

A casa virou o primeiro círculo, com os 2 usuários que já existiam.

### A tabela que não nasceu

O plano previa `exemplar`: uma cópia de uma caixa dentro de um círculo. Medido
antes de escrever — **746 caixas com pôster** (114 séries + 632 avulsas) — e a
decisão de **uma cópia por caixa** transformaria isso em 746 linhas dizendo
todas `copias = 1`.

Uma tabela cujas linhas não carregam informação é enfeite de schema. A escassez
de uma cópia virou um **índice único parcial** sobre o empréstimo em aberto:

```sql
CREATE UNIQUE INDEX emprestimo_uma_copia_work_idx
    ON emprestimo (circulo_id, work_id)
    WHERE devolvido_em IS NULL AND work_id IS NOT NULL;
```

Duas linhas de DDL no lugar de uma tabela e de uma checagem no código — e quem
recusa o segundo aluguel passa a ser o banco, sem corrida entre conferir e
inserir. É o argumento do §5 pra `CHECK` em vez de validação na aplicação, e
este é o primeiro código do projeto onde duas pessoas disputam a mesma linha de
propósito. O dia em que uma caixa precisar de duas cópias, `exemplar` nasce
carregando informação de verdade.

### A condição da fita já estava no banco

Este é o achado que sustenta a fase inteira. `playback_state` guarda, por
usuário e por obra, onde a pessoa parou. **Quem assistiu até o minuto 47 e
devolveu deixou a fita no minuto 47** — isso é literalmente verdade, não
simulação.

| na loja | no banco |
|---|---|
| voltou rebobinada | `position_seconds = 0` |
| voltou no meio | `position_seconds > 0` |
| voltou até o fim | a regra do §8f: `finished` **ou** passou de 92% |
| quem deixou assim | `playback_state.user_id` |

Uma condição de fita sorteada seria enfeite. Uma que é o progresso real de
outra pessoa da casa é **informação vestida de objeto** — que é a definição do
quarto pilar. Conferido nos três casos com dado real do acervo: *Drive* voltou
`no-meio` (60,9%), *Cassino Royale* `terminada`, *Avatar* `rebobinada`.

**O que é congelado, e por quê.** `devolvido_como` é gravado no instante da
devolução em vez de derivado na hora de exibir. Quem devolveu pode reassistir
amanhã, e aí "voltou no meio" já teria sido reescrito por um progresso
posterior. O histórico não pode depender do presente de outra pessoa.

E o 0.92 não é literal novo: é o mesmo §8f que a curadoria e o guia usam. Um
número solto aqui faria três telas do mesmo produto discordarem sobre a palavra
"terminada" — o defeito exato que a R18 desenterrou (§30). Há teste pra isso.

### Até onde o bloqueio vale

**Decidido: o empréstimo barra dentro da locadora. A biblioteca, a busca e o
`▸ assistir` continuam abertos.**

É decisão, não omissão. Barrar o player transformaria um membro em porteiro do
servidor do outro, e trancaria o dono fora do próprio arquivo — que é o cenário
que o §22 chama de regra inventada. A locadora é um lugar com regra; o disco
continua sendo seu. A escassez é honesta porque é social, e escassez social se
resolve socialmente: pedindo de volta.

A caixa alugada **continua clicável** pela mesma razão. É abrindo que se
descobre com quem ela está e que dá pra pedir. Uma caixa que não responde ao
clique seria a parede que o círculo existe pra evitar.

### O impasse, e a válvula da válvula

Bloqueio de verdade tem uma falha de modo que "compromisso visível" não tinha:
**quem esquece de devolver tranca a outra pessoa pra sempre.** Numa casa isso se
resolve gritando pelo corredor; num círculo, não. E a solução já era parte do
tema, o que é um bom sinal: **uma locadora tem prazo.** Vencido, a fita volta
sozinha.

A varredura roda **na leitura da prateleira**, não num daemon. É o padrão da
emissora (§25), que programa três canais sem tabela e sem job: quando a resposta
é calculável na hora, um processo de fundo é uma peça a mais pra quebrar.

E a válvula tem uma válvula: **o prazo não interrompe quem está assistindo.**
Fita que vence às 21h04 com a pessoa no minuto 40 devolve quando a sessão
acabar. Não é gentileza — é a mesma escolha do cancelamento cooperativo do §12,
que espera o ponto seguro em vez de matar no meio. E "a sessão acabou" tem sinal
real, não suposição: o heartbeat de 10 s do player parar de chegar. Conferido
nos dois sentidos — com heartbeat de 0 s a fita vencida **fica**; com 3 minutos
ela volta, marcada `devolvido_por = 'prazo'` e `atrasada`.

**Pedir de volta não encurta prazo de ninguém.** É registro e aviso. Dar a um
membro poder sobre o prazo do outro transformaria a locadora em disputa — e a
decisão de barrar só é defensável porque a escassez é social.

### Rebobinar, e o formato que virou comportamento

**Só em VHS.** O DVD não rebobina — ele lembra onde parou, e é por isso que ele
tem menu. A diferença que a R8 usou só como estética (lombada de papel contra
lombada de plástico) virou **comportamento**: o backend recusa com *"isto é um
DVD — ele não rebobina, ele lembra onde parou"*.

**Só a sua própria posição.** A ideia original previa rebobinar a fita de outra
pessoa, o que seria a primeira ação destrutiva entre usuários deste projeto. A
locadora não precisa disso: quem devolve rebobina, e quem não rebobinou fica
registrado no empréstimo. O fato social é guardado sem que ninguém possa apagar
o progresso alheio.

**`finished` não é tocado**, e a omissão é deliberada. Ele virou acumulativo na
§31 justamente por responder *"você já terminou isto alguma vez?"* — pergunta
sobre o passado, que rebobinar não desfaz. Rebobinar apaga onde a fita está,
não o que já aconteceu.

E o gesto é destrutivo, então a regra do §22 vale inteira: **o botão diz o que
apaga, antes.** A confirmação nomeia o minuto — *"A fita está em 0:47:00.
Rebobinar apaga o 'continuar de onde parou'"* — e a animação mostra o ponteiro
voltando de 47:00 a 0:00, no tempo. O contador não reusa o `duracao()` da ficha
de propósito: aquele arredonda pra "2h14", e um contador que pula de 47min pra
46min não parece rebobinar.

### Duas colisões de nome que a fase criou

Palavras que eram livres deixaram de ser no instante em que a devolução virou
fato, e as duas teriam ficado mentindo em silêncio:

- **A estante "Devoluções"** mostrava "continuar assistindo". Agora existe uma
  pilha de fitas que voltaram de verdade — então ela devolveu o nome e virou
  **"Começadas"**. Duas coisas diferentes não podem ter a mesma placa.
- **O botão "devolver à estante"** só fechava o palco. Com um `devolver` de
  verdade do lado, virou **"voltar à estante"**.

### Um número que mudou de lado

`ULTIMO_ANO_VHS` era constante do `Locadora.tsx`. Deixou de ser quando o mesmo
1996 passou a decidir se uma caixa rebobina: agora ele mora no backend e é
servido em `prateleira.ultimo_ano_vhs`. É a lição do §30 aplicada antes de doer
— só que o sintoma teria sido pior que o botão que dizia "ver as 644": uma caixa
desenhada como VHS que recusa o rebobinar.

### O buraco que a migração deixaria

A 0021 semeou os usuários que existiam. Um usuário criado **depois** abriria a
locadora e receberia 403 — a tela inteira quebrada por uma linha que ninguém
sabia que precisava existir. Enquanto a decisão for "só a casa por enquanto",
**estar no servidor é estar nela**, e o schema não deve discordar: quem chega
sem círculo entra no mais antigo. O dia em que houver convite, esse `INSERT`
sai e o convite entra no lugar.

### Dois defeitos que só a tela mostrou

1. **A cinta sumia na caixa grande.** A faixa de papel com o nome de quem
   levou existia só na estante — pegar uma caixa alugada fazia a marca
   desaparecer justo quando ela vira o objeto principal da tela, e a caixa na
   mão passava a parecer disponível.
2. **O botão oferecia rebobinar uma fita já rebobinada.** `caixa.posicao` vem
   da estante, e a estante só recarrega na próxima visita — o balcão recarrega,
   as 746 caixas não. A posição virou estado local do palco.

E uma escolha de apresentação que veio junto: na mão, a capa **não** escurece
como escurece na estante. Ali a caixa é o objeto principal, e uma capa apagada
lê como "carregando" em vez de "está fora". A cinta já diz isso, e diz melhor.

### Verificação

Contra o acervo real, com Firefox por Marionette (o método do §23) e com dois
usuários de verdade — sam e rudney:

| | |
|---|---|
| migração `0021` | aplicada; "A casa" criada com os 2 membros |
| escassez | rudney pegou *Drive*; sam recebeu **403 "rudney está com esta"** |
| limite | 4ª caixa recusada: *"você já está com 3 — devolva uma antes de pegar outra"* |
| condição | `no-meio` · `terminada` · `rebobinada`, os três derivados de `playback_state` real |
| caixa de série | *As Visões da Raven* devolveu `terminada`, pelo último episódio mexido |
| pedir de volta | 200 com `{"pedido_a":"sam"}`; segundo pedido e auto-pedido recusados |
| prazo | fita vencida de rudney voltou sozinha, `prazo` + `atrasada` |
| **a válvula** | com heartbeat de 0 s a fita vencida **fica**; com 3 min ela volta |
| rebobinar | DVD recusado; VHS zerou 1 linha, e `finished` não foi tocado |
| na tela | balcão, cintas, "pegar emprestado", "pedir de volta", devolver, confirmação e ponteiro |
| a 760px | nada estoura na horizontal |
| testes | **170 passam**, 5 novos |

O que ficou de fora, e é o próximo: **a prateleira finita e a rotação por
círculo** (R20). Hoje a loja continua mostrando as 600 caixas de uma vez, que é
a parede que a R8 deixou aberta — e agora que o círculo existe, o hash da
emissora (§25) só precisa dele no lugar do dia.

E uma dívida nova, pequena e registrada: o balcão do círculo **não tem tela de
administração**. Prazo e limite moram em colunas de `circulo` com padrão 7 e 3,
e mudá-los é `UPDATE`. Vira formulário quando houver um segundo círculo pra
querer números diferentes — antes disso seria tela pra ninguém.

## 36. R20 — a prateleira finita, e a loja que vira na segunda

> **Revisto em 03/08/2026.** A rotação é **estoque de loja**, e escasso: cerca
> de **40 caixas na loja inteira** — não 16 por estante, 166 no total. O que não
> está no estoque não existe até o estoque virar.
>
> E os números (tamanho do estoque, prazo, limite por pessoa, escassez ligada ou
> não) são pra ser **opções no menu do servidor**, não constantes de binário.
>
> O círculo sai do hash da rotação junto com o resto — ver §35, e §44 pra o que
> foi feito: a semente é só a semana, e a vitrine passou a ser a mesma pra todo
> mundo.
>
> **Refeito na R29 (§45):** o corte deixou de ser por estante — 40 caixas na loja
> inteira, sorteadas de uma vez —, a caixa alugada some da prateleira, e os
> quatro números ganharam tela na aba `admin`.
>
> Ver `IDEIAS.md` §3.2.

A R8 (§20) deixou um problema aberto e a R19 (§35) o deixou maior: a locadora
mostrava **600 caixas de uma vez**. Seiscentas caixas não são uma loja, são uma
parede — e uma parede é o oposto de curadoria por restrição, que é o terceiro
pilar. Um limite de três empréstimos por pessoa não faz ninguém escolher nada se
a prateleira for infinita.

### O truque já existia, e é o da emissora

A grade dos três canais da casa (§25) é `md5(dia || canal || id)` calculada no
banco: sem tabela, sem daemon, determinística. **A rotação da locadora é o mesmo
truque com a semana no lugar do dia, e o círculo junto:**

```sql
row_number() OVER (PARTITION BY estante ORDER BY md5($semente || id::text))
-- semente = segunda-feira || circulo_id
```

Duas visitas na mesma semana veem a mesma loja, em qualquer aparelho, sem nada
pra sincronizar nem pra expirar. Segunda-feira a estante vira sozinha. Medido:
duas leituras seguidas devolveram a **mesma assinatura**, e trocando só o
círculo, **0 de 16** caixas coincidiram.

**E é aqui que o círculo ganha razão de existir antes de haver empréstimo
nenhum:** entrar num círculo novo é entrar numa locadora que tem outro acervo na
vitrine.

**A semana começa na segunda, e na meia-noite local.** Não é "sete dias desde
que você entrou": uma janela deslizante por usuário faria duas pessoas do mesmo
círculo verem lojas diferentes no mesmo dia, que é o oposto do que a rotação por
círculo existe pra fazer. O fuso vem do mesmo `deslocamento()` da emissora, que
virou público por isso — duas leituras de fuso divergindo fariam a loja virar
num horário e a grade noutro.

### O corte, e de onde saiu o número

**16 caixas por estante.** Medido na própria tela: a caixa tem 130px e a
fileira 26px de intervalo, e uma estante de largura cheia mostra pouco mais de
oito. Dezesseis são **duas telas** — uma estante que se percorre com um gesto,
não um corredor. O resultado: de 600 caixas, **166 expostas**.

### As estantes mudaram de lado, e não foi arrumação

`ESTANTES` — quais gêneros formam qual estante, e em que ordem elas reivindicam
os títulos — era uma constante do `Locadora.tsx`. Foi pro backend por uma razão
de **correção**:

> a rotação corta cada estante em 16, e o corte tem que acontecer **depois** de
> cada título ser reivindicado por uma estante só.

Cortando no cliente, um título eliminado da estante que o reivindicou não
reaparece na seguinte — ele simplesmente some da loja. Reivindicar e cortar são
a mesma decisão, e decisão só pode morar num lugar.

De brinde, **uma requisição no lugar de doze**: a tela pedia `/api/library` uma
vez por estante e juntava as respostas. Agora é `GET /api/locadora/estantes`,
uma consulta, **0,32 s**. É o mesmo movimento que o guia fez em §30 ("uma
requisição e não seis") — e as duas vezes a economia foi consequência, não
motivo: a razão foi a decisão ter dono.

### A placa diz "16 de 113"

Um número que esconde o total é o "Biblioteca 300" que a R3 (§14) corrigiu: sem
o segundo número, a pessoa conclui que a loja tem 16 filmes de terror. E quando
tudo cabe na estante o "de" some — dizer *"3 de 3"* é ruído. O cabeçalho faz o
mesmo em cima: *"166 caixas na vitrine desta semana, de 600 nas estantes"*.

**E a vitrine diz quando vira.** *"vira amanhã"*, *"vira segunda"*. Sem essa
linha a rotação leria como sorteio, e uma caixa que sumiu leria como defeito —
a promessa é o que separa uma vitrine de um bug.

### O defeito que a rotação criou, e que só apareceu ao juntá-la com a R19

Uma caixa emprestada aparecia na tela como **cinta sobre a caixa da estante**.
Isso funcionava enquanto a estante tinha tudo. A partir do momento em que ela
mostra 16 de 113, a fita que rudney levou pode simplesmente **não estar exposta
esta semana** — e some da tela. Com ela some o "pedir de volta", que é a única
saída do bloqueio da R19. O bloqueio voltaria a ser parede, por um caminho que
nenhuma das duas fases teria produzido sozinha.

A correção é o que uma locadora sempre teve: **o balcão mostra o que está
fora.** A estante "Em mãos" vem antes da vitrine, independente da rotação, e é
por ela que a caixa continua alcançável. Custou três colunas na resposta do
empréstimo — arte, cor e ano — porque uma caixa desenhada fora da estante
precisa de capa. A da série desce até um episódio pra achar pôster, já que a
coleção-série costuma vir sem arte própria.

### O que a rotação **não** esconde

Vale dizer em voz alta, porque é a fronteira que torna o corte aceitável:

- **a biblioteca e a busca continuam com tudo.** A locadora é um lugar com
  regra; o acervo é o seu disco. É a mesma fronteira que a R19 (§35) traçou ao
  decidir que o empréstimo barra na locadora e não no player;
- **"Começadas" não roda.** O que você começou continua alcançável, esteja ou
  não na vitrine da semana;
- **"Em mãos" não roda**, pelo motivo acima.

"Lançamentos" passou a significar **o que há de mais novo entre o que está
exposto esta semana**, e isso é de propósito: é o que a placa de uma locadora
sempre quis dizer.

### Verificação

| | |
|---|---|
| `/api/locadora/estantes` | 200 em **0,32 s**, uma requisição no lugar de doze |
| corte | **166 expostas de 600**, 16 por estante, 12 estantes |
| determinismo | duas leituras seguidas, assinatura idêntica |
| por círculo | trocando só o círculo, **0 de 16** coincidem |
| por semana | trocando só a semana, **1 de 16** coincide (o esperado por acaso) |
| virada | `2026-08-03T03:00:00Z` — meia-noite de segunda em UTC−3 |
| placas | *"Terror 16 de 113"*, *"Faroeste 3 títulos"* — o "de" some quando cabe |
| **"Em mãos"** | *Cassino Royale*, **fora da vitrine desta semana**, visível e pedível |
| a 760px | nada estoura na horizontal |
| testes | **175 passam**, 5 novos (segunda-feira, meia-noite local, semente, placeholders, ordem das estantes) |

### O que fica de dívida

O corte é constante do binário (`CAIXAS_POR_ESTANTE`) e a estante não tem "ver
todas". Quem quiser os outros 97 filmes de terror vai pela biblioteca, com o
filtro de tag que existe desde o M2 — que é resposta legítima, mas obriga a
trocar de tela. Um "ver a estante inteira" que leve pra biblioteca já filtrada é
o passo óbvio, e não entrou aqui porque ainda não sei se a falta incomoda: a
rotação semanal pode ser suficiente pra que ela nunca se faça sentir.

## 37. R21 — o menu de DVD, e a medição que redesenhou a tela

> **Refeito na R31 (§47).** Os dois bugs relatados foram corrigidos — o clima
> agora sai da mesma ordem de reivindicação da locadora, e a grade rola —, a cena
> de fundo virou sorteada, a grade virou "capítulos" numerados, e a experiência
> 2004 (vinheta pulável, vídeo dentro dos itens, viagem de câmera até o submenu,
> trilha costurada, estilo por clima) foi construída.
>
> **Revisto em 03/08/2026.** O esqueleto serve; a experiência não. Foi pedido
> **muito mais**: um menu de DVD clássico de verdade, com alma, na referência
> da **edição especial de 2004** — vinheta animada antes do menu, vídeo rodando
> dentro dos itens, transição própria por submenu, trilha em loop costurado, e
> o **estilo saindo da temática do filme**.
>
> Mais: a cena de fundo é pra ser **aleatória** (aqui ela é sempre um quinto da
> duração), e há **dois bugs relatados** — a música sai igual em todos os
> filmes, e a lista de capítulos não rola.
>
> A troca de "capítulos" por "cenas", decidida por medição nesta seção, **não
> foi confirmada** por quem decide.
>
> Ver `IDEIAS.md` §3.7.

A ideia mais cara da lista, e a que mais dependeu de medir antes de desenhar.
O `IDEIAS.md` §3 tinha uma exigência explícita — *"medir a cobertura de
capítulos antes de desenhar a tela"* — e a medição derrubou duas premissas do
plano.

### As três medições

Nos **548 filmes identificados** deste acervo:

| | |
|---|---|
| com capítulos | **74 — 13,5%** |
| com **nomes** de capítulo úteis | **9 — 1,6%** |
| com folha de sprites (§8d), o "plano B" previsto | **0** |
| mediana de capítulos, quando há | 16 |
| recorde | 94 capítulos num filme só |

**A primeira premissa que caiu: o menu de capítulos.** Ele funcionaria em nove
filmes. Os "títulos" dos outros são vazios, `Chapter 01` — ou, o caso mais
traiçoeiro e o mais comum, **o próprio timecode repetido no campo de nome**.
Exibir `00:12:46` como se fosse o nome do capítulo é o "inglês" chutado que o
§18 recusa: parece informação, e é o mesmo número que já está do lado.

**A segunda: o plano B não existia.** O §3 assumia a folha de sprites como
saída — *"que existe pra 725 arquivos"*. Existe: **635 episódios, 88 vídeos do
YouTube e 2 clipes. Nenhum filme.** E gerá-la custa **412 s por filme**, porque
varre o arquivo inteiro.

### A saída veio de um fato que o projeto já tinha medido

O §8g estabeleceu que **`-ss` antes do `-i` é seek instantâneo**. Medido agora,
no acervo real:

| | |
|---|---|
| um quadro no minuto 30 | **724 ms** |
| doze quadros, em sequência | 5,8 s |
| doze quadros, em paralelo | 4,1 s |
| ladrilho por varredura (o método da folha de sprites) | **412 s** |

Setecentas vezes mais barato. Uma grade de doze cenas custa quatro segundos,
pagos **uma vez por filme** e guardados em disco — e cobrados só de quem entra
na tela de cenas, que num DVD também era um item de menu e também demorava um
instante pra carregar.

### O desenho que a medição impôs

> **A grade de cenas é o principal, e o capítulo é uma âncora melhor quando
> existe.**

Isso não é degradação — é o que "scene selection" sempre foi num disco: uma
grade de miniaturas com timecode. Nome de capítulo era raro até nos discos
prensados. **A diferença entre os dois casos é invisível na tela, e isso é
correto**; o que muda é só a legenda, que diz a verdade: *"nos capítulos do
disco"* ou *"em intervalos regulares"*.

Duas regras sobre onde cortar, e as duas saíram de olhar o resultado:

- **com capítulos, eles são amostrados, não truncados.** O recorde do acervo é
  94 capítulos; pegar os doze primeiros daria doze cenas do primeiro ato.
  Amostrado, a última cena de *Independence Day* cai aos **85% do filme**;
- **sem capítulos, o passo regular evita os extremos.** Os primeiros 4% são
  logo de estúdio e os últimos 4% são créditos — um não é cena, e o outro
  entrega o final.

E o capítulo que começa em zero nunca vira cena: em todo disco ele é a tela
preta antes do logo.

### O menu não se mete no caminho de ninguém

O risco que o `IDEIAS.md` §4.4 tinha apontado é real: *"um menu que atrasa o
play sem informar nada é a intro que todo mundo pula"*. A R10 (§22) já tinha
movido o "tocar" pra uma decisão consciente, mas ali havia sinopse pra ler.

Então o menu **não é uma etapa a mais** — ele é onde a caixa aberta já leva. O
disco no palco da R11 (§23) passa a abrir o menu em vez de ir direto pro
player, e o `▸ assistir` da biblioteca, da busca e da ficha continua indo direto
pro filme, como sempre foi.

**E só o disco tem menu.** A fita vai direto: ela não tem menu, tem rebobinar.
A R19 (§35) transformou a diferença de formato em comportamento de um lado; esta
é a outra metade da mesma moeda, e as duas juntas fazem VHS e DVD serem coisas
diferentes de verdade, não só estéticas diferentes.

### A música: zero bytes, e historicamente correta

O §12 recusou CDN de fonte e ficou com a serifa do sistema, *"zero bytes"* — e a
mesma régua vale pro som. Um loop `.ogg` por gênero custaria ~200 KB cada, mais
escolher e licenciar. **Web Audio custa zero bytes**, e é o que aqueles menus
eram de verdade: sequenciados, não gravados.

O gênero vira parâmetro — escala menor e raiz grave pro terror e pro suspense,
maior pro resto. Um pad de duas ondas desafinadas atrás de um filtro baixo, e um
arpejo por cima. Três decisões que só aparecem ao escrever:

- **o arpejo é agendado em blocos de oito**, não nota a nota. Agendar tudo
  encheria a fila do `AudioContext`; agendar a cada nota dependeria do
  `setTimeout` chegar na hora, e ele não chega;
- **entra e sai em fade.** Um menu que começa a tocar de estalo assusta, e
  fechar o contexto no meio de uma nota estala;
- **o som tem interruptor, e ele é lembrado.** Áudio inesperado é hostil, mesmo
  quando é a alma da coisa.

### O fundo é a emissora, de novo

A cena que roda atrás não precisou de nada novo: é **uma sessão HLS com offset
e sem áudio**, que é o que a emissora (§25) faz desde a R13. Três decisões:

- **o offset é um quinto do filme.** Não é zero e não é sorteado: um menu que
  mostra o logo do estúdio atrás não mostra o filme, e um que mostra o terceiro
  ato entrega o final;
- **só começa depois de 900 ms.** Abrir e fechar o menu num gesto não deve
  deixar um ffmpeg pra trás;
- **o backdrop do M1 aparece na hora, por baixo.** Sem ele o menu abriria em
  preto por um segundo, e a espera leria como travamento.

Na tela de cenas o fundo recua — desfoca e escurece. A cena em movimento é a
alma do menu principal e é ruído atrás de doze miniaturas; um DVD fazia o
mesmo.

### Duas correções que a tela cobrou

1. **28 legendas viraram 8 idiomas.** `subtitle_langs` é uma faixa por linha, e
   *Independence Day* traz `por, por, eng, spa, spa, fre, fre…`. Um menu que
   lista o mesmo idioma cinco vezes está mostrando faixas, não idiomas — e a
   pergunta que alguém faz na frente de um menu é *"tem português?"*. A ordem é
   preservada, porque neste acervo o português é quase sempre a primeira faixa
   e ordenar o jogaria pro meio.
2. **O menu não escolhe legenda.** Ele diz o que o disco tem; escolher continua
   no player, onde funciona desde o §18. Dois seletores seriam dois lugares pra
   manter iguais por uma escolha que já tem dono.

### Verificação

Contra o acervo real, com Firefox por Marionette:

| | |
|---|---|
| `/api/works/{id}/menu` | **0,21 s** |
| `/api/works/{id}/cenas`, primeira vez | **3,7 s** — doze extrações |
| `/api/works/{id}/cenas`, com cache | **0,018 s** |
| caminho com capítulos | *Independence Day*, 57 capítulos, 12 cenas até 85% do filme |
| caminho sem capítulos | *Drive*, passo regular de 4:01 a 1:28:41 |
| o menu | título, *Continuar 1:01:07*, *Do começo*, *Cenas* — e nada de "Legendas" onde não há |
| navegação | setas movem o foco na lista e na grade 4×3; enter escolhe; esc volta |
| a caixa | DVD abre o menu; VHS continua indo direto pro filme |
| cache em disco | 76 KB por filme, em `artwork/cenas/{media_file_id}/` |
| testes | **182 passam**, 7 novos |

### Duas dívidas, e uma armadilha que não disparou

**A armadilha:** as cenas moram dentro de `artwork_dir`, e a limpeza de órfãos
do §27 apaga o que não reconhece — foi exatamente assim que a R17 quase apagou a
foto de todos os programas (§28). Aqui ela não dispara, porque a varredura só
olha arquivos no nível de cima e as cenas estão em subpasta. Conferido com o
ensaio: 6 órfãos apontados, nenhum deles cena.

**A consequência disso é a primeira dívida:** por não serem vistas, elas também
nunca são limpas. Apagar uma obra deixa as cenas dela para sempre. São 76 KB por
filme — 40 MB se o acervo inteiro for visitado — então é dívida registrada, não
urgência.

**A segunda:** não há aquecimento. A primeira pessoa a abrir a tela de cenas de
cada filme paga 3,7 s. O molde existe e está pronto — é o `job` do §34, que já
faz exatamente isso pra trivia — e a diferença é que este não depende de rede
nenhuma. Entra quando incomodar.

**E o que não entrou de propósito:** "Extras". Um menu de DVD tinha, e este
acervo não tem nada que sirva de extra — nenhum making-of, nenhum comentário.
Um item de menu que abre uma tela vazia é pior que a ausência dele, e é a mesma
regra do §24 que faz "Continuar" sumir quando não há de onde continuar.

## 38. R22 — a ficha de produção, e a terceira peça de schema que não nasceu

O `IDEIAS.md` §0 tinha medido a ausência e proposto o conserto numa frase:
*"Um guia por região é um `ALTER TABLE` mais uma revisita ao TMDB."* A revisita
estava certa. O `ALTER TABLE` não.

### O que foi medido antes de escrever

Em 40 filmes sorteados do acervo:

| | |
|---|---|
| país de produção | **100%** |
| idioma original | **100%** |
| empresa produtora | 100% |
| orçamento e bilheteria | 92% |

**Cobertura total, e mesmo assim metade não entrou.** Porque cobertura não é o
que decide — distribuição é:

```text
países : US 34 · GB 9 · JP 3 · FR 2 · IT 1 · KR 1 · CA 1 · DE 1
idiomas: en 37 · ja 2 · ko 1
empresas distintas: 34 — em 40 filmes
```

### Três recusas, cada uma com um motivo medido

**Empresa produtora não entra.** Quase uma por filme. Um eixo em que cada item
tem uma obra não é eixo, é lista — a mesma reprovação que o corte de "2+ obras"
do §8h aplica às pessoas, e a mesma razão pela qual a R18 (§30) recusou o eixo
de produção.

**Orçamento e bilheteria não entram**, apesar dos 92%. O §33 já os traz do
Wikidata **com a moeda**, e lá foi decisão explícita que valor em moeda que não
sabemos nomear não vira curiosidade. Os campos `budget` e `revenue` do TMDB são
número puro, **sem moeda nenhuma** — escrever "US$" sobre um orçamento em euros
é a mentira com cara de metadado que o §18 proíbe. Duas fontes para o mesmo
fato, e a segunda pior, é pior que uma fonte só.

**Idioma não vira eixo** — e esta só apareceu depois de rodar no acervo
inteiro. São 547 marcações e **519 são "inglês"**: 94,9%. Dos 11 idiomas, oito
têm um filme só. Uma gaveta com 95% e oito com um filme cada não é um eixo. A
tag continua existindo e é útil — `lang:japonês` acha os 12 pelo filtro que já
existe desde o M2 — mas ela não ganha tela, exatamente como gênero e década não
ganharam na R18 (§30).

### A peça de schema que não nasceu

País e idioma viram **tags**, não colunas. O `tag`/`work_tag` do M2 já é
exatamente isto, o filtro por tag de `/api/works` existe desde então, e o guia
já resolve gênero e década por ele. Uma coluna `pais` exigiria um caminho de
consulta novo pra responder a mesma pergunta que `genre:Terror` já responde —
e o eixo de região no guia acabou sendo o SQL de gênero com um `namespace`
diferente.

Vale registrar o padrão, porque é a **terceira fase seguida** em que a medição
desfaz uma peça de schema prevista no plano:

| fase | previsto | o que nasceu |
|---|---|---|
| R19 (§35) | tabela `exemplar` | um índice único parcial |
| R21 (§37) | folha de sprites como plano B | `-ss` antes do `-i`, e cache em disco |
| R22 | `ALTER TABLE` para país e idioma | tags, no mecanismo que já existia |

A única migração desta fase acrescenta `producao` ao `CHECK` de `job.kind` —
os oito valores anteriores repetidos, pela nota que o 0013 deixou escrita e o
0020 repetiu.

### A dívida do §8, paga

O `IDEIAS.md` §8 avisava: *"A revisita ao TMDB da R22 são 548 chamadas — pela
terceira vez um reparo de minutos vai correr dentro de um request."* Não
correu. `POST /api/maintenance/aquecer-producao` nasce como `job`, no molde do
§34: estado no banco, progresso visível, cancelamento no ponto seguro, e
retomada pelo `WHERE`.

**A retomada é exata e não custou coluna nenhuma:** o alvo é "filme
identificado que ainda não tem tag de país". Rodar de novo continua de onde
parou — o mesmo truque do `repair-series` (§21).

E as requisições são **sequenciais** de propósito. 548 chamadas de ~0,2 s dão
pouco mais de dois minutos; paralelizar economizaria um minuto e transformaria
um job educado num raspador. É a mesma postura que o §33 fixou com o Wikidata.

**O filme que casar de agora em diante já nasce com a ficha**, no
`apply_candidate` — uma requisição a mais por filme **aceito**, não por
candidato avaliado, porque `production_countries` não vem no resultado da busca.
É a mesma forma que os créditos já usavam ali do lado. O aquecimento existe
pros 548 que casaram antes, não pra ser o único caminho.

### O resultado, no acervo inteiro

548 de 548, zero falhas, **33 países e 11 idiomas**. E a forma do acervo:

```text
Estados Unidos 491 · Reino Unido 92 · Alemanha 27 · França 22
Canadá 21 · Japão 20 · Austrália 11 · China 9 · Hong Kong 8 …
```

**Estados Unidos são 89,6% do acervo**, e é aí que o eixo tinha um problema de
apresentação real: uma lista ordenada por contagem começa com um número que
afoga os outros vinte e dois, e a seção passa a dizer *"você tem filme
americano"* — que é verdade e não é informação.

Duas decisões resolveram, e nenhuma delas é esconder o topo:

- **o corte de 2 obras**, o mesmo do §8h: dos 33 países, **10 têm um filme só**.
  Um país com uma obra não é prateleira, é linha de tabela — e dez delas
  empurrariam pra fora os 23 que rendem;
- **a legenda diz a forma, não o tamanho**: *"54 fora dos Estados Unidos"*. Esse
  é o número que faz a seção valer a pena, porque é a pergunta que ninguém
  conseguia fazer antes desta fase. Omitir os Estados Unidos "pra melhorar o
  eixo" seria mentir por omissão; pôr o contraste ao lado é dizer a verdade
  inteira.

### Verificação

| | |
|---|---|
| aquecimento | **548 de 548**, 0 falhas, ~2 min, retomável |
| tags criadas | 33 `country:` · 11 `lang:` |
| `/api/guia` | 0,82 s, agora com 23 países |
| o eixo na tela | de *Estados Unidos 491* a *Suécia 2*, com pilha de pôsteres |
| o clique | *Japão* → biblioteca filtrada, **20 filmes**: Akira, Ghost in the Shell, Ju-on, Noroi, O Chamado |
| testes | **184 passam**, 2 novos |

### O que fica

**A wiki ganhou o eixo que faltava**, e a lista de eixos do §30 fecha: direção,
elenco, trilha, gênero, década — e agora região. Produção continua fora, por
medição e não por esquecimento.

**Uma dívida pequena:** só filme tem ficha. Série teria que vir de `/tv/{id}`,
que devolve `origin_country` em vez de `production_countries` — campo diferente,
semântica parecida mas não igual, e as 115 séries deste acervo não sustentariam
sozinhas nenhum eixo novo. Entra quando alguém sentir falta.

## 39. R23 — a nota, e o número que a impede de mandar

> **Revisto em 03/08/2026.** A nota e o peso limitado continuam de pé. O que
> falta é a **review de verdade**: texto, com **comentário de outras pessoas**.
>
> A review mora na ficha do filme, permanente — e o feed recebe um **post de
> referência** apontando pra ela, junto com as outras atividades (deu nota,
> terminou o filme).
>
> Ver `IDEIAS.md` §3.4.

Uma fase pequena decidida por uma frase. O `IDEIAS.md` §4.6 tinha aprovado
classificação e resenhas **com ressalva**, e a ressalva era a fase inteira:

> sinal fraco não pode mandar no forte.

O M5 nasceu de duas premissas — *"nada é declarado"* e *"terminar > assistir"* —
porque nota é enviesada: as pessoas dão cinco estrelas pro que acham que
**deveriam** gostar, e uma estrela por raiva do final. Deixar a nota pesar mais
que o comportamento desfaria o M5 inteiro, e o desfaria em silêncio: a curadoria
continuaria respondendo, só que errado.

### O número, e por que é 0,3

A escala de afinidade do §8f já existia e não mudou:

```text
terminou              1.0
gostou (≥60%)         0.6
neutro                0.1
largou cedo (≤15%)   −0.8
reassistir           +0.2 / +0.4
```

`PESO_DA_NOTA = 0.3` **não foi escolhido pelo gosto** — é o maior valor que não
inverte nada:

```text
terminou  1.0  +  nota 1 (−0,3)  =  0,7   → continua positivo
largou   −0.8  +  nota 5 (+0,3)  = −0,5   → continua negativo
```

A nota move a obra **dentro da faixa que o comportamento já determinou**, e
não atravessa o zero. Três estrelas valem exatamente zero: "achei ok" não é
informação a favor nem contra, e tratá-lo como qualquer uma das duas seria
inventar opinião.

Isso está travado por teste, e o teste explica por que existe — um peso maior
quebraria a regra sem quebrar nada visível, e o defeito só apareceria quando o
`/for-you` começasse a recomendar o que a pessoa abandonou.

**Conferido no acervo real**, que é diferente de conferido no teste: cinco
estrelas em *A Casa de Cera* — largado aos 0% — e o perfil continuou contando
**12 largadas**. O comportamento mandou.

### Duas coisas que a nota não faz

**Não cria gosto sozinha.** A consulta do perfil parte do `play_event`, então
uma nota só chega à curadoria acompanhada de um sinal de comportamento. Avaliar
um filme que você nunca abriu não move nada — a nota qualifica o que você
assistiu, não substitui o assistir.

**Não se mistura com o comportamento no perfil inspecionável.** O §4.6 exigiu os
dois separados, e `avaliadas` e `nota_media` são campos próprios em
`/api/curation/taste`. Misturados, não daria pra responder *"o Odeon está me
recomendando pelo que eu vi ou pelo que eu disse?"* — e essa pergunta é a razão
de o perfil ser inspecionável.

### Por que não reusar o `work_feedback` do M5

Era tentador: a tabela existe, tem zero linhas, e guarda opinião. São coisas
diferentes, e o verbo denuncia:

| | o que é | fala com |
|---|---|---|
| `work_feedback` | **instrução ao recomendador** — "nunca mais me ofereça" | o sistema |
| `avaliacao` | **julgamento sobre a obra** — "isto é um 4" | você e a casa |

Um `block` some com a obra do `/for-you`; uma nota 2 não deve sumir com nada. E
dá pra amar um filme que você avaliaria 3. Fundi-las forçaria uma a mentir.

### A nota da casa, e não a do mundo

O que a ficha mostra é **o seu círculo** — a R19 (§35) rendendo de novo. A média
de estranhos é o IMDb com passos extras, e disso o mundo já tem; a nota de
alguém que você conhece diz alguma coisa.

O filtro por círculo hoje não muda nada (todo mundo está na casa) e passa a
mudar no dia em que a R25 trouxer gente de fora. É a mesma jogada do
`programme.work_id` do §17: escrever a cláusula agora custa uma linha e evita
descobrir depois que a ficha estava mostrando a nota de qualquer conta do
servidor.

### Escolhas de tela, cada uma com o seu porquê

- **Cinco estrelas, não dez nem meia.** Meia-estrela dá impressão de precisão
  que ninguém tem sobre um filme. Cinco degraus é o que uma locadora usava.
- **Clicar numa estrela já grava.** Exigir "salvar" pra uma nota transformaria
  um gesto em formulário. O texto, que de fato precisa de confirmação, tem o
  botão dele.
- **O texto é opcional.** A maior parte das avaliações do mundo é só a nota, e
  exigir prosa faria a nota não ser dada. Texto em branco vira ausência de
  texto, não texto vazio — senão a ficha renderizaria um parágrafo de nada.
- **"Tirar a nota" existe.** "Não sei mais o que achei" é estado legítimo, e a
  alternativa — ficar preso numa nota antiga — faria a nota não ser dada.
- **A parte apagada da estrela é a mesma estrela sem cor.** Assim a fileira tem
  sempre cinco e o olho lê a proporção, não a contagem.

### O defeito que só a leitura do texto mostrou

A fileira de estrelas **mentia pra quem não vê**. Com cinco glifos sempre
presentes — três acesos, dois apagados —, o texto extraído de uma nota 3 saía
`★★★★★`, e um leitor de tela diria "cinco estrelas". A correção é um
`aria-label` com o número e as estrelas marcadas `aria-hidden`: a proporção
continua sendo desenhada, e quem lê ouve *"3 de 5"*.

É o §18 num lugar inesperado — a tela estava dizendo com cara de informação uma
coisa que não era verdade.

### Verificação

| | |
|---|---|
| dar, trocar e tirar nota | 200 nos três; `PUT` porque avaliar de novo é trocar de ideia |
| texto só com espaços | vira `null`, não parágrafo vazio |
| nota fora de 1–5 | 400 |
| obra apagada entre abrir e avaliar | 404, e não 500 |
| a ficha | *"O que a casa achou · 4.0 · 2 notas"*, com a sua marcada e a do outro abaixo |
| perfil inspecionável | `avaliadas` e `nota_media` em campos próprios |
| **a invariante, no acervo** | 5 estrelas num filme largado aos 0% → **continua largado** |
| testes | **189 passam**, 5 novos |

### O que fica

**A medição que vale registrar:** este acervo tem **5 obras com sinal** de
comportamento suficiente pra render opinião. A R23 não trouxe volume — trouxe a
possibilidade, e a regra que impede o volume futuro de estragar o M5. É o
oposto do erro da R15 (§26): a tela não está confiante sobre o que não sabe,
porque ela não afirma nada; quem afirma é quem avalia.

**Uma dívida pequena:** nota só existe pra obra. Avaliar uma **temporada** ou
uma série inteira é uma pergunta diferente, e o `work_id` da chave já aceitaria
o id de uma coleção se o tipo mudasse — mas mudar o tipo pra atender uma
pergunta que ninguém fez ainda é o `exemplar` da R19 de novo. Entra quando
alguém quiser dar quatro estrelas pra terceira temporada.

## 40. R24 — a retrospectiva, e o placar que entra reprovado

> **Desfeito na R32 (§48).** O placar saiu do produto — arquivo, rota e tela —,
> e no lugar dele entrou o que foi pedido: 72 conquistas em seis camadas, XP
> derivado, nível, títulos e tags desbloqueáveis, bio, vitrine e a comparação
> com os amigos **dentro** do perfil. A retrospectiva ficou.
>
> A separação que esta seção defendeu era real: apagar o placar custou um
> arquivo e uma linha, como previsto. O preço foi a feature ficar escondida
> numa aba que ninguém abria.
>
> **Revisto em 03/08/2026, e este é o mais importante de corrigir.**
>
> O argumento contra gamificação registrado nesta seção — *"não passa nos
> pilares"*, o aviso impresso na tela mandando ignorar o número — **não é
> posição do projeto. É posição de quem escreveu, contra quem decide.**
>
> O que foi pedido é um sistema ao estilo das conquistas da Steam: lista longa
> em camadas (nível, fáceis, médias, difíceis, impossíveis, sagas), XP, nível
> de usuário, comparação com amigos, tags e customização de perfil. A
> retrospectiva foi uma **substituição não pedida** — ela pode sobreviver como
> tela de perfil, mas não no lugar das conquistas.
>
> Ver `IDEIAS.md` §3.3.

Duas telas, dois módulos, duas rotas — e a separação **não é organização, é
reversibilidade**. O `IDEIAS.md` §6.2 decidiu "os dois, separados" com uma
frase que é a especificação inteira desta fase:

> o dia em que o placar estiver escolhendo filme por você é um dia em que dá pra
> desligar só ele e ficar com a parte que descreve.

### A premissa da fase estava errada, e a medição disse na cara

A §7 apostou: *"agora existe atividade pra descrever: aluguéis, devoluções,
atrasos, notas, fitas rebobinadas"*. Medido antes de escrever:

| | |
|---|---|
| eventos | 128 |
| obras tocadas | 18 — **2 terminadas, 12 largadas** |
| pessoas com afinidade | 15 |
| **empréstimos** | **0** |
| **avaliações** | **0** |
| dias com atividade | 3 |

**Não existe.** A R19 e a R23 construíram o motor; ninguém rodou ele ainda. Os
empréstimos e as notas que apareceram nas verificações daquelas fases eram
testes, e foram removidos por serem exatamente isso.

Isto é a armadilha da R15 (§26) — a tela que não estava crua, estava
*"confiante sobre o que não sabia"* — chegando na tela que mais tenderia a
repeti-la. A defesa não podia ser editorial ("escrever com cuidado"), porque
daqui a um mês haverá dados e ninguém vai reler o texto. Ela é estrutural:

> **cada bloco só existe quando tem o que dizer.**

Hoje a retrospectiva rende **4 blocos e cala 2**. Quando a locadora rodar, rende
6. Nenhuma linha precisa mudar pra isso acontecer.

E o rodapé diz quantos calaram: *"2 capítulos ficaram de fora por não terem o
que contar ainda."* Sem isso, uma tela curta faz a pessoa concluir que o Odeon
não sabe nada dela — que é a leitura errada de uma tela que está sendo honesta.

### A retrospectiva é o perfil do M5 com roupa nova

Nada aqui é declarado, nada é pontuado, nada premia volume. O §8f já sabia
dizer *"você costuma terminar palestra (100%)"*; a fase só deu voz a isso.

O bloco de abertura é o que mais define o tom:

> **Você abriu 18 obras e terminou 2.** Larga bem mais do que termina — é gente
> que experimenta, não que coleciona.

Um painel de gamificação esconderia as 12 largadas porque elas "não pontuam".
Aqui elas são metade do retrato, e a metade mais interessante.

Duas escolhas de honestidade que o dado cobrou:

- **a hora vira período.** O histograma tem um pico, não um compromisso —
  *"você assiste mais de madrugada"* é verdade; *"às 23h"* seria preciso e
  falso. E histograma zerado devolve `None` em vez de "meia-noite", que é a
  diferença entre não saber e afirmar;
- **só afinidade positiva.** *"Você odeia documentário"* é informação boa pra
  curadoria e uma frase que ninguém pediu pra ler sobre si. A retrospectiva
  descreve; não julga.

### O defeito da primeira versão: uma frase que o dado não sustentava

Ela dizia *"15 pessoas aparecem mais de uma vez no que você termina"* — e
`person_affinity` conta obras **abertas**, não terminadas, e inclui quem você
largou. A frase afirmava mais do que sabia.

Corrigido filtrando por afinidade positiva, e aí o número mudou junto: **6
pessoas**, e agora o título ("quem você não larga") e a frase dizem a mesma
coisa que o dado. É o §18 aparecendo numa tela em vez de num metadado —
inventar não é privilégio de campo de banco.

### O placar entra reprovado, e o custo está na própria tela

O §5 aplicou os quatro pilares a onze ideias e **o placar foi a única
reprovação que sobreviveu**. Ele entra por decisão explícita, e o §6.2 mandou
registrar o custo. Registrá-lo só no doc deixaria de fora justamente quem está
olhando pro número — então ele está impresso embaixo do placar:

> Contar não é medir. Um filme de 4 minutos vale o mesmo que um de 3 horas aqui
> — se este número começar a escolher o que você assiste, ignore-o.

Quatro decisões que reduzem o dano sem inventar nada:

- **o streak não quebra hoje.** Se a última atividade foi ontem, ele continua
  contando: o dia ainda não acabou, e um contador que zera às 00h01 transforma
  a noite numa obrigação. Ele só quebra quando um dia inteiro passa em branco;
- **o maior streak sobrevive à quebra**, senão a tela apagaria o que a pessoa
  fez por causa de uma viagem — a punição do §6.2 na forma mais crua;
- **as horas são a posição máxima alcançada**, não a duração das obras
  terminadas. Quem parou aos 40 minutos assistiu 40 minutos; contar o filme
  inteiro premiaria abrir e fechar;
- **zero em tudo não vira painel de zeros.** `tem_o_que_contar` faz a tela
  dizer *"ainda não há o que contar"* em vez de imprimir `0 · 0 · 0`, que
  parece defeito e ensina a não voltar.

### As três regras do §6.2, e onde cada uma é verdade no código

Não bastava obedecê-las: era preciso que dessem pra **conferir**.

| regra | onde ela vive |
|---|---|
| a retrospectiva nunca cita o placar, e vice-versa | dois módulos e dois componentes; nenhum importa o outro. As únicas menções cruzadas são comentários explicando a regra |
| o placar não entra em nenhuma tela do fluxo principal | sala própria em `experimentação`; `ForYou` não o conhece |
| o placar não alimenta o M5 | `placar.rs` é **somente leitura** — não tem um `INSERT`, `UPDATE` ou `DELETE` — e `curation/` não o importa |

**Desligar o placar é apagar dois arquivos e duas linhas.** Se um dia alguém
precisar de um número que já está calculado ali, a resposta certa é duplicar a
consulta, não criar a dependência.

### A armadilha que a própria verificação encontrou

Ao conferir a regra 2 por `grep`, o resultado deu **positivo**: `ForYou.tsx`
continha "placar". Era um falso positivo com um problema de verdade dentro —
uma fileira de lâmpadas da calibração usava `className="placar"` desde antes,
pra mostrar quantas obras você já votou.

Nada visualmente colidia. Mas o §6.2 exige que dê pra desligar o placar
sozinho, e quem for fazer isso vai começar por um `grep` — que encontraria um
indicador de calibração no meio do fluxo principal e concluiria a coisa errada.
A classe virou `.calib-luzes`. **A separação precisa sobreviver ao grep, não só
à intenção.**

### Verificação

| | |
|---|---|
| `/api/retrospectiva` | 4 blocos, **2 calados**, no acervo real |
| a frase de abertura | *"Você abriu 18 obras e terminou 2…"* |
| pessoas | 6 com afinidade positiva, com retrato |
| gostos | Japão · japonês · Crime — e os dois primeiros são tags da R22 (§38) |
| `/api/placar` | 2 terminadas · 5 horas · 3 dias · streak 3 |
| as três regras | conferidas por `grep`, não por leitura |
| lâmpadas da calibração | 6, intactas depois do renome |
| testes | **196 passam**, 7 novos |

### O que fica

**A dívida é de dado, não de código.** As duas telas estão prontas pra uma
locadora que rodou e para notas que foram dadas; nenhuma delas precisa mudar
quando isso acontecer. É o oposto do erro da R15: em vez de uma tela confiante
sobre o que não sabe, duas telas que sabem exatamente o tamanho do que têm.

**E uma observação que só a fase seguinte responde:** a R25 (feed do círculo)
depende do mesmo combustível que faltou aqui. Ela vai encontrar `play_event` com
128 linhas de uma pessoa só, e zero empréstimos. O motor existe desde a R19 —
falta alguém pegar uma fita.

## 41. R25 — o mural do círculo, e o que ele se recusa a contar

> **Revisto em 03/08/2026.** O que esta seção chama de decisão de privacidade —
> *"o mural conta o que terminou, não o que abriu"* — contraria a visão:
> **entre amigos é tudo aberto**, inclusive o que está sendo assistido agora e
> o que foi largado no meio.
>
> E o mural é uma fração do que foi pedido. Falta postar, comentar, pesquisar
> pessoas, ver quem está online (duas listas: servidor e amigos), mandar mensagem
> e customizar perfil.
>
> **Feito em parte na R28 (§44):** o mural deixou de ser do círculo e passou a
> ser seu — você e seus amigos —, e adicionar amigo existe.
>
> **Concluído na R33 (§49):** a poda por privacidade caiu, e com ela o resto da
> lista — posts, comentários, presença, busca e mensagem direta. Dos dois
> motivos que esta seção deu pra podar, o de privacidade perdeu a premissa (o
> aceite da amizade é o consentimento) e o de **volume** ficou de pé: as fontes
> novas são uma linha por pessoa, não o log cru.
>
> Ver `IDEIAS.md` §3.8.

A última da lista, e o `IDEIAS.md` §7 já sabia por quê: *"o último porque é o
que mais depende de tudo acima ter gerado acontecimento"*.

### A medição, e o que ela obrigou a decidir

| | |
|---|---|
| `play_event` cru | **128 linhas** |
| pares (pessoa, obra) | 18 |
| **obras terminadas** (§8f) | **2** |
| empréstimos · avaliações | **0 · 0** |
| membros do círculo | 2 — e **1 com histórico** |

Um mural sobre a primeira linha diria *"sam abriu Drive"* dezoito vezes. Não é
mural, é log. Sobre a terceira, são duas entradas. **Nenhuma das duas é um feed
ainda** — e essa era a informação que a fase precisava encarar em vez de
disfarçar.

### A decisão: conta o que terminou, não o que abriu

Ela resolve dois problemas, e o segundo não estava no plano.

**Volume.** 128 vira 2. Um mural de casa que registra cada play é ruído, e
ruído ensina a não olhar — a mesma razão do §24.

**Privacidade.** Um feed cru publicaria pra casa inteira tudo que cada um abriu
e largou aos oito minutos. Isso é **mudança de contrato**: até aqui
`play_event` era privado, e a R19 (§35) só tornou público o que acontece
*entre* as pessoas — quem pegou uma fita, quem devolveu como.

Terminar é diferente de experimentar. Anunciar o que se completou é o que
alguém contaria na cozinha; anunciar cada coisa que se provou e abandonou é
vigilância com cara de recurso social. O §8f já dizia que **terminar é o sinal
honesto**; aqui ele vira também **o limite do que se publica**.

Vale dizer o que isso custa, porque é uma escolha e não um teorema: as 12
obras largadas deste acervo não aparecem no mural. Elas aparecem na
retrospectiva (§40), que é sua e só sua — e essa assimetria é o desenho, não um
descuido.

### Cinco acontecimentos, nenhuma tabela

O §6.5 tinha previsto: *"o feed é um `SELECT` sobre `play_event` e `emprestimo`
com um `JOIN` — nada de segurança muda"*. É literalmente isso, com `avaliacao`
junto: um `UNION ALL` de cinco fontes, escopado por `circulo_membro`.

```text
rudney avaliou Drive — ★★★★                                      hoje
Você terminou 007: Cassino Royale                                hoje
rudney devolveu Drive — sem rebobinar · atrasada · pelo prazo   ontem
Você pediu Drive de volta — de rudney                       há 3 dias
rudney pegou Drive na locadora                          há uma semana
```

A terceira linha é a fase inteira num exemplo: *"sem rebobinar · atrasada ·
pelo prazo"* são três fatos sobre uma pessoa real, nenhum deles inventado — a
condição saiu do `playback_state` de quem teve a fita (§35), o atraso saiu da
comparação com o prazo, e "pelo prazo" saiu da devolução automática.

**É a quarta fase seguida em que a peça de schema prevista não nasce.** O §38
já tinha registrado três — `exemplar` (R19), a folha de sprites (R21), as
colunas de produção (R22) — e o feed fecha o padrão.

E o `terminou` sai da regra do §8f, que agora tem **cinco leitores**: curadoria,
guia (§30), locadora (§35), placar (§40) e mural. Uma definição, cinco telas —
escrever outra aqui faria o mural discordar da retrospectiva sobre a palavra.

### A frase é montada no cliente, e é a exceção que confirma a regra

As curiosidades (§32) e a retrospectiva (§40) montam a frase no servidor, e o
motivo sempre foi o mesmo: uma gramática só. Aqui é o contrário, de propósito.

Estas cinco frases são **gramática de lista**, não prosa: o servidor manda o
tipo e as peças, e a tela conjuga. Mandar a frase pronta impediria o mural de
dizer *"Você"* no lugar do seu próprio nome — e "sam terminou Cassino Royale"
lido pelo sam é a diferença entre um mural e um relatório sobre você.

### O mural diz quantas vozes tem

Com os dados reais, ele mostra duas linhas e depois:

> Só uma das 2 pessoas do círculo apareceu por aqui até agora.

Um mural com um nome só não é a casa conversando, é uma pessoa em voz alta.
Sem esse número a tela pareceria funcionar tendo só um lado — e é exatamente o
estado deste acervo. É a defesa da R24 (§40) outra vez: **a tela diz o tamanho
do que ela tem**, em vez de deixar o vazio parecer completude.

### Duas escolhas menores, e o porquê de cada uma

- **as suas linhas ficam marcadas, não escondidas.** Um feed que esconde os
  próprios atos é o padrão da indústria e conta uma história torta: você é
  membro do círculo, e metade do que aconteceu na casa foi você;
- **lista, não cartões.** Cartão dá peso igual a tudo e transforma "rudney
  devolveu sem rebobinar" em anúncio. Um recado de corredor se lê de cima pra
  baixo.

E o barramento do M3 alimenta o mural como já alimenta a locadora desde a R19 —
o que acontece na loja aparece aqui sem recarregar. É o que separa um mural de
um relatório.

### O balcão e o mural mostram devolução: por que os dois

A locadora (§35) já lista as últimas devoluções no balcão. Não é duplicata: são
perguntas diferentes. O **balcão** é o que está acontecendo na loja agora —
quais fitas estão fora, o que acabou de voltar, e é lá que se pede de volta. O
**mural** é a história do círculo, com tudo que aconteceu e não só o que a
locadora produziu.

Se um dia as duas discordarem, quem está errado é o balcão: o mural lê as
tabelas cruas, o balcão lê um recorte.

### Verificação

| | |
|---|---|
| `/api/feed` | 200; escopo por `circulo_membro` |
| os cinco tipos | conferidos com dados temporários: terminou · pegou · devolveu · pediu · avaliou |
| a devolução | *"sem rebobinar · atrasada · pelo prazo"*, os três fatos da R19 |
| o pedido | *"Você pediu Drive de volta — de rudney"* |
| **com os dados reais** | 2 linhas, e *"só uma das 2 pessoas apareceu"* |
| tempo | relativo (*hoje*, *ontem*, *há uma semana*), não data exata |
| limite | grampeado em 200 — `?limit=100000` num UNION de cinco fontes seria varredura a pedido |
| testes | **198 passam**, 2 novos |

Os dados usados pra provar os cinco tipos foram removidos: eram empréstimos e
notas inventados por mim, atribuídos a pessoas reais.

### O que fica, e é o fim da lista do `IDEIAS.md`

**A R18 à R25 estão feitas.** O que sobra do plano é o que ele mesmo mandou pra
projeto próprio: círculo federado entre Odeons (§6.5), com seção de segurança
própria.

**E fica a observação que atravessou as três últimas fases:** R23, R24 e R25
construíram telas para uma atividade que ainda não existe. Elas não estão
quebradas nem cruas — estão **corretas e vazias**, cada uma dizendo o tamanho
do que tem. A locadora funciona desde a R19; falta alguém pegar uma fita.

Isso é o oposto do erro que a R15 (§26) registrou, e é a coisa da qual este
documento tem mais orgulho nesta série: a tela que não sabe, diz que não sabe.

## 42. R26 — o convidado, e a auditoria que o precedeu

> **Revisto em 03/08/2026.** Duas coisas.
>
> **A fase inteira nasceu de uma pergunta de quem programa**, não de quem
> decide — "federado ou hospedado?" foi proposta aqui, e a lista de ideias
> original não pede nem um nem outro.
>
> **E a rota de presença fechada abaixo como vazamento é feature.** A visão
> pede que amigo veja o que amigo está assistindo agora; o `/api/transcode/
> sessions` virou rota de admin justamente pra impedir isso. O token de mídia
> do §43 continua bom e fica.
>
> **Resolvido na R28 (§44):** o convite passou a ser do servidor e `guest`
> sobreviveu — a regra "só assiste o que pegou emprestado" perdeu o JOIN com o
> círculo e não perdeu mais nada.
>
> **E a presença voltou na R33 (§49)** — mas não por esta rota. O
> `/api/transcode/sessions` continua fechado, e o motivo mudou: ele só enxerga
> quem está transcodificando, e o §3 decidiu que aqui o caso comum é Direct
> Play. A presença de verdade sai de `auth_session.last_seen_at` e do heartbeat
> do player.
>
> Ver `IDEIAS.md` §2.2 e §4.

O `IDEIAS.md` §6.5 adiou "gente de fora" e mandou o assunto pra projeto próprio,
*"com seção de segurança própria"*. Esta é a seção.

### A auditoria, feita com uma conta de verdade

O §6.5 listou três compromissos assumidos por causa da tailnet: o `?token=` na
query (§9b), o CORS pelo mesmo host (§10b) e a montagem gravável (§22). Medido
com a conta `user` que já existe neste servidor, o buraco é maior que a lista:

| o que uma conta comum alcançava | rota | veredito |
|---|---|---|
| os caminhos do seu disco | `/api/libraries` → `/media/Movies` | corrigido |
| o mapa das montagens, e que é gravável | `/api/storage` → `gravavel: true` | virou rota de admin |
| quem está assistindo o quê, agora | `/api/transcode/sessions` | virou rota de admin |
| **o acervo inteiro, sem escopo** | `/api/stream/{qualquer}` → **206** | é o resto desta seção |

Os três primeiros são vazamentos comuns e o conserto é chato. O quarto é o
interessante, porque **não era um bug**.

### A decisão da R19 estava certa, e é exatamente errada aqui

A R19 (§35) decidiu que o empréstimo barra **na locadora e não no player**, e o
raciocínio continua de pé: barrar a reprodução transformaria um morador em
porteiro do outro, e trancaria o dono fora do próprio arquivo.

Para um forasteiro a mesma regra se inverte de sentido: entrar no círculo
entregaria a prateleira inteira, e a escassez — a cópia única, o prazo, o pedir
de volta — viraria encenação.

Isso é literalmente o que o §6.5 quis dizer com *"a rede social muda a ameaça,
não só a tela"*. A diferença é que agora há medição em vez de suposição.

### A regra nova, e o que ela faz a R19 valer

> **O convidado só assiste o que pegou emprestado.**

| papel | assiste |
|---|---|
| `admin`, `user` — moradores | **tudo**. O disco é deles |
| `guest` — convidado | **só o que está com ele**, enquanto estiver |

Nada disso foi inventado nesta fase. A cópia única da R19 vira verdade técnica;
o prazo vira o fim do acesso; a devolução automática vira a revogação. E a
revogação **não precisou existir**: `devolvido_em IS NULL` é a autorização
inteira, então devolver — por gesto ou por prazo — corta o acesso no mesmo
instante, sem nenhum caminho separado pra alguém esquecer de escrever.

Conferido de ponta a ponta, com uma convidada de verdade:

```text
sem empréstimo  → 403 "você precisa pegar esta caixa emprestada na locadora"
pega a caixa    → 206
devolve         → 403
```

### A regra tem um dono, e é um arquivo

`auth/acesso.rs`. Toda rota que entrega **bytes de mídia** passa por ele:
stream direto, plano de reprodução, sessão HLS, legenda, folha de sprites, menu
de DVD e a grade de cenas. Espalhar a checagem pelos handlers é como o defeito
nasce — seis lugares, e o sétimo esquece.

Duas das sete não eram óbvias e valem nota: a **folha de sprites** (§8d) é o
filme inteiro em miniatura, e a **grade de cenas** (§37) são doze quadros dele.
Servir qualquer uma a quem não pegou a caixa é entregar o conteúdo em resolução
baixa e chamar de metadado.

### O `session_id` era a autorização inteira

`GET /api/hls/{session_id}/{arquivo}` servia os segmentos pra quem tivesse o id.
Um UUID não é adivinhável — mas **id impalpável é capacidade, não permissão**, e
é a mesma ressalva que o §9b já fazia sobre o `?token=`. Com um convidado no
círculo ela deixa de ser acadêmica.

A sessão passou a ter dono, e `hls_file` responde **404** — e não 403 — pra
sessão alheia: quem pede não deveria saber que ela existe. `stop_session` ganhou
a mesma trava, porque encerrar a sessão de outra pessoa derruba a reprodução
dela.

As sessões de canal ao vivo (§25) têm dono `nil`: elas nascem da emissora e não
de um pedido de pessoa. São da casa, e morador as alcança.

### O que a regra deliberadamente NÃO restringe

**Navegar.** Um convidado lê o acervo inteiro — título, sinopse, elenco, pôster,
a locadora com a rotação da semana. Uma locadora deixa ler a caixa toda antes de
alugar, e um catálogo que esconde o que existe não é loja, é cofre com vitrine.

É escolha, não descuido: convidar alguém é dizer a essa pessoa o que você tem.
Quem não quiser dizer, não convida.

### O convite

Código de 128 bits — o mesmo teto do token de sessão do §9b — guardado como
SHA-256, como as sessões desde sempre. Vazar o banco não dá convite a ninguém.

Ele **vence em sete dias**, o mesmo prazo da fita (§35), e a coincidência é de
propósito: as duas coisas são empréstimos de acesso. Um convite eterno é uma
senha permanente esquecida num aplicativo de mensagem.

Três detalhes que a tela cobra:

- **o código aparece uma vez só**, e o aviso vem junto — não depois, que é
  quando a pessoa já fechou a janela;
- **resgate e login são o mesmo gesto**. Criar a conta e em seguida pedir pra
  digitar o mesmo usuário e senha de novo seria cerimônia;
- **a mesma frase pra código errado, vencido e usado.** Distinguir diria a quem
  tenta se o código existe, e um "não" que informa é um oráculo.

### A colisão de nome, que desta vez tinha consequência de acesso

A tela de administração já tinha um botão **"+ convidar"** em *Pessoas*. Ele não
convida: ele **cria um morador**, com senha definida ali, e morador assiste
tudo.

Com o convite de verdade logo abaixo, ficaram dois botões com o mesmo rótulo e
consequências de acesso opostas — e é assim que um estranho vira morador por
engano. Virou **"+ criar conta"**, e a opção do papel passou a dizer o que ela
faz: *"morador — vê e assiste tudo"*.

É a segunda vez nesta série que um nome herdado atrapalha uma separação nova; a
primeira foi o `.placar` da R24 (§40). A diferença é que lá o risco era de
manutenção e aqui é de permissão.

### O defeito que a própria limpeza encontrou

Ao remover a convidada de teste, o banco recusou:

```text
ERROR: new row for relation "convite" violates check constraint
       "convite_uso_completo"
```

Duas regras que eu tinha escrito **no mesmo arquivo, com dez linhas de
distância**, se contradiziam:

- `usado_por … ON DELETE SET NULL` — apagar a pessoa não deve apagar o registro
  de que o convite foi usado;
- `CHECK (usado_em e usado_por preenchidos juntos, ou nenhum)`.

O `SET NULL` produz exatamente o estado que o CHECK proíbe. O resultado era uma
linha de `app_user` **indelével**, e o administrador descobriria isso por um
erro de constraint no meio da tela.

O CHECK é que estava errado: *"usado por alguém que não está mais aqui"* é um
estado legítimo — é o que resta de um convidado removido. A 0026 troca a regra
por `usado_por IS NULL OR usado_em IS NOT NULL`, que proíbe só a metade que
nunca faz sentido.

A lição vale mais que o conserto: **uma constraint que descreve o estado normal
pode proibir o estado residual que outra regra produz de propósito.**

### Verificação

| | |
|---|---|
| convite → conta | emitido, resgatado, e o **mesmo código recusado na segunda vez** |
| a convidada, sem empréstimo | 403 em stream, plan, menu, cenas e scrub |
| a convidada, com empréstimo | **206** |
| depois de devolver | **403**, sem revogação em separado |
| o morador | inalterado — 206 sem empréstimo, como o §35 decidiu |
| `/api/libraries` | `root_path` só pra admin; a lista continua alimentando o filtro |
| `/api/storage`, `/api/transcode/sessions` | 403 pra quem não administra |
| remover um convidado | funciona (0026) |
| testes | **203 passam**, 7 novos |

### O que fica de dívida, e é honesto dizer

**O `?token=` na query continua lá.** Ele é o primeiro dos três compromissos do
§6.5, e esta fase não o resolveu: `<video src>` e `<img src>` continuam sem
mandar header. O §9b já escreveu o conserto certo — *"emitir um token de mídia
curto e separado do token de sessão"* — e ele ficou de fora porque o escopo por
empréstimo já limita o estrago: um token de convidado vazado num log serve
apenas para o que aquela pessoa pegou emprestado, e só enquanto pegou.

**A montagem gravável também continua** (§22). Um convidado não alcança nenhuma
rota que escreva em disco, mas a montagem é `rw` e o conserto de verdade é o
`:ro` que o próprio `docker-compose.yml` documenta como reversível.

**E o círculo federado continua não existindo.** Esta fase escolheu o convidado
hospedado, que o §6.5 chama de *"um Netflix com dono"* — e a alfinetada é justa.
A diferença é que agora o dono empresta em vez de servir: o convidado não tem
acesso ao acervo, tem acesso ao que pegou.

## 43. R27 — o token de mídia, e a dívida mais antiga do projeto

O `auth/middleware.rs` carregava esta frase desde o M6:

> Token em query string vaza pra log de acesso e histórico do navegador.
> Restringi-lo às rotas de mídia limita o estrago, e num servidor de uma pessoa
> só na tailnet o risco é aceitável. **Se um dia isso for exposto de verdade, o
> certo é emitir um token de mídia curto e separado do token de sessão.**

O §6.5 listou esse `?token=` como o primeiro dos três compromissos que "gente de
fora" cobraria. A R26 (§42) trouxe gente de fora e **não pagou**. Esta paga.

### O que exatamente estava errado

O que ia na query **era o token de sessão**: 90 dias de validade, acesso total à
API. Um `access.log` de proxy reverso, um histórico de navegador, um print de
tela com a URL do vídeo — qualquer um deles entregava uma conta inteira, não um
pôster.

### As oito horas, medidas antes de escolhidas

Um token de mídia precisa sobreviver a assistir a coisa mais longa do acervo sem
interrupção; se ele vencer no meio, a reprodução quebra e a fase vira um
incômodo. Medido:

| | |
|---|---|
| arquivo mais longo | **4,90 h** |
| filme mais longo | 4,04 h — *Liga da Justiça de Zack Snyder* |
| arquivos acima de 3 h | 15 |
| **acima de 5 h** | **0** |

Oito horas cobrem o maior arquivo com três de pausa por cima, e são **1/270 da
validade da sessão**. Menos quebraria a reprodução no meio; mais desfaria a
razão de a fase existir.

### O que faz a separação valer alguma coisa

Emitir um token novo não conserta nada sozinho — se a query continuasse
aceitando o de sessão, isso seria cerimônia. O conserto está no middleware:

> **header e cookie resolvem sessão; a query resolve mídia.**

São duas tabelas e dois escopos, e o caminho por onde o token chegou decide qual
consultar. Conferido nos quatro quadrantes:

| | |
|---|---|
| sessão na query | **401** — era 200 antes desta fase |
| sessão no header | 200, a API inalterada |
| mídia na query | 200 |
| **mídia no header, contra a API** | **401** — ele não lista, não aluga, não administra |

Há um teste que trava isso, e ele lê o próprio código-fonte do `require_auth`
pra verificar a ordem dos ramos. É feio de propósito: se alguém um dia reunir os
três caminhos num `or_else` de novo, o token de sessão volta a valer na URL e a
fase inteira desaparece **sem nenhum teste de comportamento quebrar**.

### Por que tabela, e não um token assinado

Um HMAC dispensaria a tabela e a consulta. O §9b já enfrentou essa escolha ao
recusar JWT pra sessão — *"JWT é stateless, o que soa bom até você querer
deslogar um aparelho perdido"* — e aqui vale mais: um token de mídia vazado é
justamente a coisa que se quer revogar **agora**, não daqui a oito horas.

A linha some da tabela e acabou. E a consulta é um `SELECT` por chave primária
numa rota que já lê o banco pra achar o arquivo.

### Sair passou a sair

`revoke` apagava a sessão e deixava o token de mídia vivo — oito horas de bytes
depois de um logout. Agora ele lê o dono da sessão **antes** de apagá-la e
revoga a mídia junto; o cliente limpa as duas chaves pelo mesmo motivo.

Um "sair" que não sai não é sair.

### O que fica

**O `?token=` continua existindo**, e vai continuar: `<video src>`, `<img src>`
e `<track>` não mandam header, e isso não é escolha do Odeon. O que mudou é o
que está escrito ali.

E o segundo dos três compromissos do §6.5 — **a montagem gravável** (§22) —
continua de pé. Nenhuma rota que um convidado alcance escreve em disco, e o
conserto de verdade é o `:ro` que o próprio `docker-compose.yml` documenta como
reversível em uma linha. Fica registrado, sem pressa fingida.

### Verificação

| | |
|---|---|
| emissão | `POST /api/auth/media-token`, autenticada por header |
| os quatro quadrantes | 401 · 200 · 200 · 401, como a tabela acima |
| logout | o token de mídia morre junto: 200 → **401** |
| a tela | 25 imagens de artwork, **nenhuma quebrada** |
| HLS | playlist servida com o token de mídia; sessão alheia continua 404 (§42) |
| testes | **204 passam**, 1 novo — o que lê o `require_auth` |

## 44. R28 — amigos no lugar do círculo, e a tabela que morreu em vez de ser renomeada

Esta é a primeira fase escrita **depois** do realinhamento de 03/08/2026 — a
primeira em que o `IDEIAS.md` é a espec de verdade, e não uma interpretação
dela. Ela desfaz um conceito que oito fases usaram como alicerce.

### O conceito que ninguém pediu

A R19 (§35) precisava de um escopo pra escassez. O argumento estava escrito na
própria migração 0021, e ele é bom:

> *"'este DVD está alugado' é falso quando o arquivo está sempre lá, e o §18
> proíbe dizer coisa falsa com cara de metadado. Dentro de um círculo deixa de
> ser falso — a fita **está** com alguém."*

Daí nasceu o **círculo**: um grupo fechado, com dono, com prazo e limite
próprios. E como a mesma migração observou, adotá-lo "custa uma coluna e evita
uma migração dolorosa" — então ele foi adotado em seis lugares de uma vez:
empréstimo, rotação, nota, feed, convite e o acesso do convidado.

O raciocínio tinha um furo, e não é o técnico. A palavra nas anotações originais
é **amigos**. Ninguém pediu grupo. O escopo foi inventado pra resolver um
problema de honestidade do schema, e depois passou a decidir coisas de produto —
quem vê a sua nota, quem aparece no seu mural, o que um convidado assiste.

### A pergunta que desfez tudo: de quem é o estoque?

Duas saídas cabiam. Traduzir `circulo` para `grupo` e seguir; ou perguntar de
quem é a loja. A escala já estava medida no `IDEIAS.md` §3.2:

> ~40 caixas na loja inteira por semana — **não 40 por estante**.

*A loja*, no singular. Uma locadora de bairro não tem um estoque por turma de
amigos. Com o estoque sendo **do servidor**, o empréstimo deixa de precisar de
escopo, e "amigos" passa a existir só onde significa alguma coisa: no social.

Isso não é economia de código, é uma mudança de regra — **uma cópia por caixa no
servidor**, não uma por caixa por grupo. Quem te barra agora pode ser qualquer
pessoa que entra ali, e é por isso que o balcão passou a listar todas elas.

### A janela, e ela não volta

Medido antes de escrever uma linha:

| | |
|---|---|
| círculos | 1 — "A casa", semeado pela própria 0021 |
| membros | 2, que são os 2 usuários do servidor |
| empréstimos · avaliações · convites | **0 · 0 · 0** |

**Não havia um único dado escopado por círculo.** O `ALTER TABLE` custou nada;
daqui a um mês custaria decidir a que grupo pertence cada empréstimo já feito. A
0021 fez a conta certa com o sinal invertido: era a adoção que ia doer depois.

A única coisa real dentro de `circulo_membro` era que aquelas duas pessoas se
conhecem — e ela foi preservada. A migração semeia a amizade a partir da
associação que existia, em vez de deixar as duas acordarem estranhas.

### Uma linha por amizade, e o Postgres é quem ordena

O par é **canônico**: `a < b` por uuid, imposto por CHECK, com a chave primária
sendo o par. `(sam, rudney)` e `(rudney, sam)` são a mesma linha, e duas linhas
dizendo a mesma coisa — podendo discordar — são inrepresentáveis. É o argumento
do §5 outra vez: quem recusa é o banco, não uma checagem que o segundo caminho
de código esquece.

**Quem ordena é o `least()`/`greatest()` do Postgres, não o Rust.** Se o Rust
ordenasse, a comparação que produz a linha e a que o CHECK confere seriam duas
implementações da mesma coisa, e o dia em que divergissem o INSERT falharia com
erro de constraint sem ninguém entender o motivo.

Cai fora disso um caso bonito: **se os dois se pedirem ao mesmo tempo, viram
amigos.** O segundo INSERT cai no `ON CONFLICT`, vê que quem pediu foi o outro, e
aceita. Não é caso de borda tratado — é a mesma regra lida de trás pra frente.

### O aceite é o consentimento, porque não existe outro

A decisão 2.2 do `IDEIAS.md` é forte e foi tomada por quem decide: amigo vê o
que você está assistindo **agora**, o que largou no meio, o que terminou, suas
notas. **Sem chave de privacidade.**

Então o botão "aceitar" carrega tudo isso sozinho, e é por isso que ele existe —
uma amizade unilateral, tipo seguir, faria qualquer conta do servidor abrir a sua
estante sem você dizer nada. Recusar **apaga a linha** e não deixa marca: guardar
a recusa serviria pra barrar um segundo pedido, o que num servidor de duas
pessoas resolve um problema inexistente e cria um pior — quem pediu ficaria
vendo "pendente" pra sempre sem saber que já levou não.

### O que mudou de comportamento, e não só de nome

| | antes | agora |
|---|---|---|
| escassez | uma cópia por caixa **por círculo** | uma cópia por caixa **no servidor** |
| vitrine da semana | `md5(semana ‖ círculo ‖ id)` | `md5(semana ‖ id)` — **todo mundo vê a mesma loja** |
| balcão | os membros do seu círculo | as pessoas do servidor |
| notas na ficha | de quem entrou no mesmo grupo | de quem **aceitou** ser seu amigo |
| mural | do grupo, igual pra todos | seu: você e seus amigos |
| convite | pra um círculo | pro **servidor** |
| acesso do convidado | empréstimo **+** ser membro do círculo | o empréstimo, que era o que sempre autorizou |
| prazo e limite | colunas do `circulo` | `locadora_opcoes`, singleton do servidor |

A segunda linha é a que mais muda a alma da coisa. O círculo entrar no hash era o
que dava a ele razão de existir antes de haver empréstimo — cada grupo tinha sua
vitrine. Com uma loja só isso vira o contrário do que a vitrine serve: **a caixa
da semana é a mesma pra todo mundo**, e por isso é assunto em comum, do mesmo
jeito que o guia é igual pra todos (`IDEIAS.md` §2.4).

### Prazo e limite não voltaram a ser `const`

A 0021 escreveu que *"um número de regra de negócio escondido em `const` é um
número que ninguém encontra"*, e isso não deixou de valer só porque o dono das
colunas sumiu. Elas migraram pra `locadora_opcoes`, um singleton imposto pelo
banco (`unica boolean PRIMARY KEY CHECK (unica)`), herdando os valores que a casa
tinha em vez de recomeçar no padrão. É a semente da tela de opções da fase 2.

### O que esta fase deliberadamente NÃO fez

**O mural continua contando só o que foi terminado.** A decisão 2.2 reverte isso,
e o §41 registra a poda como se fosse posição do projeto — não é. Mas trocar o
escopo e o conteúdo na mesma fase esconderia duas mudanças de comportamento atrás
de um commit só. Cai na fase 6, junto com posts, comentários, presença e mensagem.

**A presença fechada (§42) continua fechada**, pelo mesmo motivo e no mesmo lugar
da fila.

### Verificação

| | |
|---|---|
| migração | 0028 aplicada; `circulo` e `circulo_membro` **não existem mais** |
| amizade preservada | 1 linha, `rudney + sam`, aceita — semeada da associação antiga |
| pedir · repetir · aceitar · desfazer | `enviado` · `ainda não respondeu` · `agora vocês são amigos` · 404 no segundo DELETE |
| pedido mútuo simultâneo | vira amizade aceita, sem passo extra |
| pedir a si mesmo | 400, `você já se tem` |
| escassez sem grupo | um usuário aluga, o outro leva **"Fulano está com esta"** |
| convidado | `guest` com a fita na mão: **206**; depois de devolver: **403**; morador: 206 |
| nota na ficha | invisível pra quem não é amigo, visível **no instante** em que a amizade é aceita |
| telas | mural e locadora conferidos por screenshot; balcão some quando não há o que dizer (§24) |
| testes · typecheck | **206 passam** (2 novos) · `tsc --noEmit` limpo |
| dados de teste | conta `r28teste` e as sessões criadas foram **apagadas**; 2 usuários, 0 empréstimos, 0 avaliações, e o último `play_event` é anterior à sessão |

## 45. R29 — quarenta caixas, e a chave que desliga a escassez sem virar um `if`

A R20 (§36) fez a coisa certa com o número errado. Ela cortou a vitrine porque
seiscentas caixas são uma parede, e o corte foi **16 por estante** — 166 caixas
expostas. A escala pedida estava escrita:

> A locadora tem **~40 caixas na loja inteira** por semana — não 40 por estante.
> O que não está no estoque não existe até o estoque virar.

166 não é uma loja pequena. É uma loja.

### O corte deixou de particionar, e isso muda a silhueta

A mudança inteira é a ausência de duas palavras:

```sql
row_number() OVER (PARTITION BY a.estante ORDER BY md5(semana || id))  -- R20
row_number() OVER (                       ORDER BY md5(semana || id))  -- R29
```

Cortar por estante dá **a mesma vitrine toda semana com conteúdo trocado**:
doze placas, dezesseis caixas cada, sempre. Isso é catálogo paginado com nome de
loja. Sorteando na loja inteira, a estante deixa de ser cota e vira endereço — é
só onde cada caixa foi cair.

Medido no acervo real, com estoque 40, na semana de 03/08:

| | |
|---|---|
| caixas | 40, em **11 estantes** |
| Ficção científica | 9 de 115 |
| Terror | 6 de 113 |
| Crime e suspense | 8 de 52 |
| Romance · Guerra · Drama | 1 cada |
| **Faroeste** | **não existe esta semana** |

O faroeste tem 3 caixas no acervo; numa semana ele aparece, na outra não. **A
loja muda de forma**, e não só de conteúdo — que é o que uma loja pequena faz.

### O número que a tela somava errado

A porta da loja dizia *"597 no acervo"* de 600, e o defeito só apareceu porque a
primeira semana sorteada não contemplou o faroeste: a tela somava os `total` das
estantes **que vieram na resposta**, e a estante ausente levou o acervo dela
junto.

É o §14 outra vez — o "Biblioteca 300" que escondia o denominador — e o §30 — o
botão que dizia "ver as 644" e abria 1.424. **Toda vez que a tela recalcula um
número que o servidor já sabe, ele diverge.** O acervo passou a vir servido,
numa janela `count(*) OVER ()` que roda antes do corte e custa um `bigint` por
linha.

### A chave de escassez, e por que ela não é um `if`

Esta é a parte que quase deu errado. A opção pedida é *"escassez ligada ou
não"*, e o jeito óbvio de implementá-la é ler a coluna no handler e pular a
inserção conflitante. Isso **desmontaria o §35**:

> *"quem recusa é o banco, não uma checagem que alguém pode esquecer de escrever
> no segundo caminho de código"*

Um `if` traz de volta a corrida entre a leitura e o `INSERT` que o índice único
existe pra matar — e traz de volta justamente no primeiro código do projeto onde
duas pessoas disputam a mesma linha de propósito.

O predicado de um índice parcial não pode consultar outra tabela. Mas pode olhar
uma coluna da própria linha. Então o empréstimo passou a **carregar o regime sob
o qual nasceu**:

```sql
ALTER TABLE emprestimo ADD COLUMN exclusivo boolean NOT NULL DEFAULT true;

CREATE UNIQUE INDEX emprestimo_uma_copia_work_idx ON emprestimo (work_id)
    WHERE devolvido_em IS NULL AND work_id IS NOT NULL AND exclusivo;
```

Três coisas saem de graça, e a terceira é a que importa:

* a regra continua sendo do banco, com a força de antes;
* desligar a escassez **não afrouxa nada retroativamente** — a fita que saiu sob
  o regime exclusivo continua exclusiva até voltar, o que é honesto: ela está
  com alguém;
* ligar de volta não invalida os empréstimos duplicados que existirem, e nenhum
  estado impossível precisa ser inventado.

**E uma regra que era subproduto virou explícita.** "Ninguém pega a mesma caixa
duas vezes" era consequência do índice acima: se só há uma cópia em aberto,
ninguém tem duas. Desligada a escassez, deixa de ser — e sem o índice novo
`emprestimo_uma_por_pessoa_*` a mesma pessoa acumularia três cópias do mesmo
filme, queimando o próprio limite.

### A caixa alugada some, e o buraco fica

A R19 desenhava a caixa emprestada na estante com uma cinta de papel por cima. A
anotação original diz outra coisa: *"se alguém aluga, a caixa **some da
prateleira** e volta quando devolve"*.

**E o buraco não é preenchido.** Puxar outra caixa do acervo pra tapar o vão
faria a loja ter 40 sempre, e aí levar uma fita não custaria nada a ninguém. O
estoque da semana é o estoque da semana.

Some quando o empréstimo **tranca** ou quando é **seu** — os dois casos em que a
caixa não está ao seu alcance. Com a escassez desligada, a de outra pessoa
continua exposta, e a cinta volta a ter função: *"fulano está com esta, e você
também pode pegar"*. A cinta não foi removida; ela mudou de lugar.

A porta da loja conta as três coisas separadamente, porque são três: **"39 na
prateleira, 1 fora · 40 nesta semana, de 600 no acervo"**. Sem o "1 fora", quem
viu 40 ontem conclui que a loja quebrou.

### A tela que faltava

Os quatro números viraram uma seção da aba `admin`, e cada campo diz **o que
muda**, não o que é. "Estoque: 40" não informa nada a quem não escreveu o
código; *"caixas expostas na loja inteira por semana — não por estante"*
informa. É a mesma lição da placa que diz "3 de 113" em vez de "3".

A validação continua sendo dos `CHECK` das migrações — repeti-la no handler
criaria dois lugares pra discordar sobre o que é um prazo válido. O que o
handler faz é traduzir a violação pra 400 em vez de deixar sair 500.

### Verificação

| | |
|---|---|
| migração | 0029 aplicada; `estoque = 40`, `escassez = true` |
| vitrine | **40 caixas em 11 estantes**; com `estoque = 8`, 8 em 6 estantes |
| faroeste | ausente na semana sorteada — e a placa não mente por isso |
| acervo | **600**, servido; era 597 quando a tela somava |
| escassez ligada | o segundo aluguel leva *"Fulano de Teste está com esta"* |
| a mesma pessoa | *"esta já está com você"*, nos dois regimes |
| escassez desligada | o segundo aluguel **passa**, e nasce `exclusivo = false` |
| o que já estava fora | continua `exclusivo = true` depois de desligar a chave |
| validação | `estoque = 0` → **400** com a frase dos intervalos; morador comum → **403** |
| a prateleira | 39 na prateleira · 1 fora · 40 na semana · 600 no acervo |
| telas | locadora e `admin` conferidas por screenshot |
| testes · typecheck | **208 passam** (2 novos) · `tsc --noEmit` limpo |
| dados de teste | conta `r29teste`, os 3 empréstimos e as sessões **apagados**; opções de volta em 40 · 7 · 3 · ligada |

## 46. R30 — a fita é um objeto, e por isso rebobinar deixou de ser destrutivo

Esta seção desfaz a recusa mais cara da série. O §35 escreveu:

> *"A ideia original previa rebobinar a fita de outra pessoa — o que seria a
> primeira ação destrutiva entre usuários que este projeto teria."*

E a recusa **estava certa**, dado o modelo que existia. O erro não era ético,
era de modelagem — e é a distinção que esta fase inteira depende de fazer.

### O achado da R19 estava certo pela metade

A 0021 comemorou, com razão:

> *"o estado da fita já está no banco. `playback_state` guarda, por usuário e por
> obra, onde a pessoa parou. Quem assistiu até o minuto 47 e devolveu deixou a
> fita no minuto 47 — isso é literalmente verdade, não simulação."*

A observação é boa. A conclusão não, e o furo só aparece quando se tenta fazer o
que foi pedido:

> **Uma fita é um objeto. `playback_state` é uma memória.**

Enquanto a fita for a memória de alguém, três coisas quebram:

* rebobinar a fita **é** apagar o "continuar de onde parou" de outra pessoa — e
  aí a recusa do §35 é a única resposta possível;
* a fita anda para trás sozinha: quem devolveu no minuto 47 e reassistiu amanhã
  reescreve o passado, e foi preciso **congelar** `devolvido_como` no empréstimo
  pra contornar isso;
* e duas pessoas têm fitas **diferentes** da mesma caixa, que é a negação do
  objeto.

Separar as duas dissolve o problema inteiro em vez de contorná-lo:

| | o que é | de quem é |
|---|---|---|
| `playback_state` | onde **você** parou | seu, privado, intocável |
| `fita` | onde **a fita** está | do acervo, compartilhado |

Rebobinar passa a mexer no objeto e em ninguém. **Nenhum `playback_state` é
tocado no handler** — foi o primeiro fato que a verificação conferiu, e ele é a
seção inteira em uma linha.

### A fita anda enquanto se assiste

Não só na devolução. É a leitura literal da segunda anotação — *"saber que
estado deixou a fita para o próximo uso"* — e a consequência importa: **levantar
no meio e sair já deixa a fita zoada**, tenha você alugado ou não. Um morador
pode dar play sem passar pela locadora (§35), e continua podendo; o que ele não
faz mais é sair sem deixar marca.

O `INSERT ... ON CONFLICT` mora no mesmo lugar que grava o progresso — o único
ponto de escrita que existe —, e um `WHERE w.year <= 1996` faz a coisa toda
virar no-op para DVD sem nenhum `if`: disco não rebobina, ele lembra onde parou.

### Você descobre quando põe pra tocar

Cena a cena, como foi pedido. O estado da fita **não** vai junto com as 40 caixas
da vitrine: é uma rota própria, chamada ao abrir a caixa e revelada só no play.
Seria mais barato em requisições mandar tudo com a estante, e destruiria a única
coisa que esta tela tem.

O que aparece não é o filme — é o descuido de outra pessoa, com nome:

> **ESTA FITA NÃO ESTÁ REBOBINADA** · `0:47:12` · *Fulano deixou assim · hoje*

O nome está ali porque é o nome que faz o atrito existir. Uma fita no meio sem
dono é um defeito do sistema; com dono é alguém que não rebobinou.

### Rebobinar é obrigatório — quando a fita não é sua

Decidido, e mais afiado que a proposta que estava no `IDEIAS.md` (que oferecia
"dar play daqui"). Não há como passar: a fita de outra pessoa se rebobina antes,
e os segundos que isso custa são o preço que o descuido dela cobra de você.

**A ressalva é a única coisa que não foi decidida e sim inferida**, e está
registrada aqui pra poder ser revertida: se foi **você** que deixou a fita no
meio, não há obrigação — isso é a sua sessão continuando. Obrigar alguém a
rebobinar o próprio filme porque saiu pra pegar água transformaria o tema em
castigo, e o preview da decisão mostrava explicitamente a fita de outra pessoa.

E os segundos são **de verdade**, proporcionais a quanto a fita andou: um segundo
para cada doze minutos, entre 2,5s e 10s. Um VHS levava perto de dois minutos pra
voltar uma fita inteira; dez segundos é a caricatura disso — longa o bastante pra
irritar um pouco, curta o bastante pra ninguém sair da sala. *"Alguns segundos de
verdade, mas sem ser massante."*

### A confirmação sumiu, e a ausência é o argumento

O botão tinha uma modal — *"rebobinar apaga o continuar de onde parou"* —, e ela
era a regra do §22 aplicada corretamente: **o botão diz o que apaga, antes de
apagar.** Agora ele não apaga nada de ninguém, e a modal não tem o que dizer.

Pedir confirmação para um gesto que não destrói é a ceninha que ensina a clicar
em "sim" sem ler — e aí a confirmação que **importa** também não é lida. A modal
saiu porque a regra que a justificava deixou de se aplicar, e não apesar dela.

### O log tem dois nomes

> *"as pessoas saberem quem devolveu zoado e ter que rebobinar"*

As duas metades da frase estão numa tabela só, e é por isso que ela guarda dois
nomes: um rebobinar é sempre **um trabalho que alguém teve por causa de alguém**.
`por` gastou os segundos, `de` deixou assim.

Ler a reputação do estado atual da `fita` não serviria: ela só sabe da última
pessoa, e esquece tudo no instante em que alguém rebobina — ou seja, esqueceria
exatamente quando o fato acabou de acontecer.

No balcão, cada pessoa carrega até três números, e **zero some** (§24):

| | |
|---|---|
| `✕n` | fitas dela que alguém teve que rebobinar |
| `⟲n` | fitas dos outros que ela rebobinou |
| `n` | quantas caixas tem na mão |

O segundo existe pra o primeiro não fazer de todo mundo réu. E quem tem fama
continua aparecendo no balcão **mesmo sem nada na mão** — a reputação tem que
sobreviver à devolução, senão ninguém carrega nada.

`rebobinar a própria bagunça` não conta pra nenhum dos dois: o log guarda o fato
inteiro, e é a **leitura** que filtra. Um log que mente por educação não é log.

### Uma armadilha que custou uma migração

A 0030 rodou pela metade: o binário do container não recompilou depois de eu
editar o `.sql`, e o `sqlx::migrate!` aplicou a versão anterior — deixando a
tabela sem uma coluna e o `_sqlx_migrations` com o checksum errado, o que faria
toda execução seguinte falhar. É a armadilha que o `CLAUDE.md` já documenta, e
ela morde mesmo quando se sabe dela.

O conserto foi derrubar as duas tabelas, apagar a linha da migração e recompilar
com `touch src/main.rs`. Só foi barato porque a fase tinha **um dia de idade**.

### Verificação

| | |
|---|---|
| migração | 0030 aplicada inteira, em segunda tentativa |
| a semente | 1 fita real — *Ghost in the Shell* (1995) no minuto 59, deixada pelo `sam`. Não é seed de exemplo |
| a fita anda | um usuário assiste até 47:12 → `fita` marca 2832s no nome dele |
| **o `playback_state` alheio** | intacto em **2832** depois do rebobinar. É a recusa do §35 dissolvida, medida |
| a fita | zerada, e `deixada_por` passa a ser quem rebobinou |
| o log | `por = sam`, `de = fulano`, `segundos = 2832` |
| `minha` | `false` pra quem chega, `true` pra quem deixou — é o que decide a obrigação |
| rebobinar de novo | `{"rebobinadas": 0}`, sem escrever log |
| DVD | 400: *"ele não rebobina, ele lembra onde parou"*; a rota da fita responde `vhs: false` |
| reputação | `fulano ✕1` · `sam ⟲1` e 1 fita no meio agora |
| a tela | conferida por screenshot, dirigida por ponteiro real do Marionette |
| testes · typecheck | **209 passam** (2 novos) · `tsc --noEmit` limpo |
| dados de teste | conta `r30teste`, o log, as duas fitas de teste e as sessões **apagados**; sobrou a fita real |

### O que fica devendo

A **animação de rebobinar a fita** que a anotação pede é hoje um ponteiro
regressivo e um carretel que anda pra trás. É honesto e é pouco: falta o objeto
girando, o ruído, o tranco no fim. Fica anotado junto com o menu de DVD (fase 4),
que é a fase do mesmo tipo de trabalho.

## 47. R31 — o menu de 2004, e o gênero que a locadora já sabia

A R21 (§37) entregou um menu de disco funcional e sem alma: fundo em movimento,
quatro itens, uma grade de miniaturas. O pedido é outro — *"realmente um menu de
DVD clássico, com alma"*, com a edição especial de 2004 como referência. Esta
fase é a diferença entre as duas coisas, mais dois defeitos relatados.

### O primeiro bug tinha resposta pronta a três arquivos de distância

> *"a música é igual em todos os filmes"*

Duas causas somadas. O gênero chegava por um `SELECT … LIMIT 1` **sem
ordenação** — o Postgres devolvia qualquer uma das até seis etiquetas do filme —
e o sintetizador reduzia isso a **três variantes**, com dois regex sobrepostos.
Medido: 218 dos 635 filmes caíam no mesmo par escala/raiz.

A locadora já tinha resolvido exatamente esta pergunta. `ESTANTES` (§36) é uma
lista **ordenada** de reivindicação, com os gêneros distintivos primeiro, e é por
isso que *Alien* mora em ficção científica em vez de drama. Usar a mesma ordem
aqui custa uma consulta com `unnest` e rende coerência de graça:

> **o filme que mora na estante de terror abre um menu de terror.**

Doze estantes viraram doze climas — escala, fundamental, andamento, timbre da
melodia, timbre do colchão e corte do filtro. Nenhum é decorativo: tons inteiros
em ficção científica porque nenhuma nota resolve; menor harmônica em crime porque
a sétima maior dentro do menor é tensão sem susto; frígio em terror porque é o
modo mais escuro que sete notas dão sem dissonância gratuita.

Conferido em seis filmes: *Suspiria* → Terror, *Aladdin* → Infantil,
*Independence Day* → Ficção científica, *Bom Dia, Vietnã* → Guerra, *Django
Livre* → Faroeste, *Drive* → Crime e suspense.

**O índice é o contrato**, e há teste: os dois arrays que viram `unnest($3,$4)`
precisam ter o mesmo tamanho — `unnest` de arrays desiguais preenche o menor com
NULL **em silêncio**, e o sintoma seria uma estante casando com o gênero errado.
O menu abriria; só com a música de outro filme.

### O segundo bug era metade CSS, metade grid

> *"a lista de capítulos não rola"*

`.menu-dvd` é `position: fixed` com `overflow: hidden`, e tem que ser — senão o
palco 3D vaza pela viewport. Faltavam duas coisas: uma área de rolagem **dentro**
(`overflow-y: auto` na grade) e um `min-height: 0` no palco, que é uma linha
`1fr` de grid e sem isso assume a altura do conteúdo. Com as duas, um disco de
trinta e sete capítulos declarados rola; sem, os últimos ficavam sob o rodapé.

### A cena de fundo era determinística

`duracao / 5.0`, fixo. Abrir o mesmo disco dez vezes dava o mesmo plano dez
vezes, e a anotação original pede *"uma cena aleatória do filme rodando de
fundo"*. Agora é sorteada **entre 15% e 75%** da duração: antes disso ainda há
logo de estúdio, depois começa o desfecho. A janela é a mesma decisão de antes —
agora com largura em vez de um ponto.

### Capítulos, e a medição que não mudou nada

Medido de novo, e o número piorou: **3 filmes de 635 têm capítulo no container, e
nenhum tem nome.** O §37 tinha 13,5%; a realidade é 0,47%.

Mesmo assim a grade passou a se chamar **capítulos**, numerada de 1 a 12. Isso
não é o §18 sendo violado: o Odeon não está dizendo que o arquivo declarou
capítulos — está **dividindo o filme em capítulos**, que é o que ele faz. A
legenda continua dizendo de onde veio o corte (*"nos cortes do disco"* contra
*"divididos pelo relógio"*), e o número no item de menu só aparece quando o disco
o declarou. O que mudou é a palavra, e a palavra é a do objeto.

### A experiência 2004, item a item

**A vinheta.** Toda vez, e pulável — é o que os discos bons faziam; os ruins eram
os que travavam o controle. Dois segundos e meio, quatro desenhos (risco, íris,
onda, brilho) servindo os doze climas, e o que sempre muda é a tinta.

E ela é **uma fase**, não uma camada: enquanto roda, o palco do menu não existe.
A primeira versão montava os dois, e o print mostrou o defeito na cara — o título
do filme e os itens legíveis por baixo da vinheta, que vira um borrão sobre um
menu já aberto. É o oposto de pôr um disco.

**Vídeo dentro dos itens.** Um `<canvas>` por item, todos pintados do mesmo
`<video>` com recortes diferentes. Quatro elementos de vídeo seriam quatro
decodificações — e aqui o fluxo é HLS transcodificado ao vivo, ou seja, quatro
sessões de ffmpeg pra mostrar o mesmo plano. Um vídeo e quatro `drawImage` de
360×72 custam uma. A janela só acende no item em foco: quatro ligadas ao mesmo
tempo viram uma parede de vídeo, e aí não há foco nenhum.

**A viagem até os capítulos.** Menu e capítulos são duas telas **no mesmo
espaço**, e a transição move a câmera. `perspective` no palco, `translateZ` e
`rotateY` nas telas: a que sai recua e desfoca, a que entra vem de trás. Antes o
React trocava o conteúdo — e trocar conteúdo é o que uma aba faz.

**A trilha costurada.** O colchão nunca para, e é ele que costura: a melodia
recomeça, o pad não tem emenda. E a frase de oito notas passou a **continuar** a
anterior em vez de recomeçar no primeiro grau — um contador de compasso, três
linhas, e o loop deixa de soar como um bloco repetido.

**O estilo saindo do filme.** A tinta do clima acende o item em foco e o número
do capítulo, e convive com `--cor` (a cor dominante do filme) em vez de
substituí-la: um menu de terror de um filme azul continua sendo daquele filme.

**Sobre a paleta.** O §12 fechou as cores do produto, e as doze tintas são uma
exceção deliberada, não um esquecimento. A decisão é explícita — *"comédia e
terror não ganham o mesmo menu"* — e um menu de disco não é cromo do aplicativo:
é a arte da edição especial, que nunca combinou com o resto da estante.

E `prefers-reduced-motion` desliga a viagem. Alma não pode custar enjoo a quem
pediu pra não ter movimento.

### Verificação

| | |
|---|---|
| clima | 6 filmes, 6 climas, cada um batendo com a estante da locadora |
| a cena | 3 aberturas do mesmo disco: 2718s · 2291s · 1692s |
| capítulos | grade numerada 1–12 com timecode, rolando; legenda diz de onde veio o corte |
| vinheta | conferida em `clima-iris` (Moana → Animação), sozinha na tela |
| menu | conferido em `clima-risco` (Pânico 2 → Terror): janela de vídeo no item em foco, tinta vermelha |
| viagem | menu → capítulos por seta e Enter, com a câmera viajando |
| testes · typecheck | **211 passam** (2 novos) · `tsc --noEmit` limpo |
| dados de teste | só sessões, todas apagadas; nada escrito no acervo |

### Duas coisas que a conferência ensinou

**O Firefox headless atrasa animação de entrada.** Três prints saíram de uma tela
com `opacity: 0` porque eu fotografava no instante do `mount`. O roteiro passou a
**esperar o elemento aparecer** e só então contar o fade — adivinhar o instante
deu print da estante três vezes.

**E o print achou um bug que o código não denunciava.** A vinheta por trás do
menu compilava, passava no typecheck e não quebrava teste nenhum. Só olhando.

## 48. R32 — conquistas, e o placar que argumentava contra si mesmo

O §40 registrou, com todas as letras, um argumento contra a gamificação:

> *"Contar não é medir. Um filme de 4 minutos vale o mesmo que um de 3 horas
> aqui — se este número começar a escolher o que você assiste, ignore-o."*

Aquilo estava **impresso na tela**, dentro da feature. E o §40 tratou o
argumento como posição do projeto quando ele era posição de quem escreveu,
contra quem decide. O pedido é explícito desde a primeira anotação: *"algo
parecido com as conquistas da Steam"* — XP, nível, camadas, muitas conquistas,
comparação com amigos, títulos e customização.

**O aviso sai.** Um produto que entrega uma feature e imprime na tela um pedido
de desculpas por ela não entregou a feature — entregou a discussão sobre ela.

### O XP é derivado, e é isso que faz tudo ser retroativo

Não há tabela de pontos, ledger nem job de recálculo. O nível de alguém é uma
**função** do que essa pessoa fez, lida na hora. Três coisas saem disso, e a
primeira era uma decisão em aberto (`IDEIAS.md` §6.3):

* **as conquistas são retroativas de graça.** Zero linhas de backfill: no dia em
  que isto ligou, quem já tinha terminado dois filmes já tinha terminado dois
  filmes;
* nada desincroniza — um contador que soma a cada evento erra pra sempre no dia
  em que um evento se perde;
* apagar um empréstimo corrige o XP sozinho, em vez de deixar pontos órfãos de
  um fato que não existe mais.

É a **quinta** fase seguida em que a peça de schema prevista não nasce (§38
registrou três, §41 a quarta). O banco guarda só o desbloqueio: a chave e o
instante.

**Medido antes, e o número tempera a expectativa:** 129 eventos, de uma pessoa
só, 18 obras, **2 terminadas**. Retroativo abriu exatamente duas conquistas — *A
primeira fita* e *Não está sozinho*. A lista foi escrita para o histórico que ela
vai criar, não para o que existe, e é honesto que ela comece quase toda trancada:
uma lista que abre cheia não é uma lista, é troféu de participação.

### Duas consultas, e não setenta e duas

Avaliar cada regra com a sua própria consulta seria uma ida ao banco por
conquista, a cada leitura de perfil. O avaliador levanta os **fatos** de uma
pessoa — contagens, máximos, sequências — em duas consultas, e as regras são
funções em cima dessa estrutura. Uma regra nova é uma linha, e só custa consulta
se pedir um fato que ainda não existe.

### A lista, e por que a camada é que vale pontos

Setenta e duas conquistas, em seis camadas. Os pontos são **fixos por camada** e
não por linha: um número por conquista seriam setenta e duas decisões
arbitrárias, e a primeira coisa que alguém faria é comparar duas e achar uma
injusta. A camada **é** a dificuldade.

| camada | pontos | pra quê |
|---|---|---|
| fáceis | 10 | dopamina — a lista não pode abrir vazia |
| médias | 40 | o corpo: pedem hábito, não façanha |
| sagas | 80 | trilogias e coleções |
| difíceis | 150 | pedem meses |
| impossíveis | 1000 | **não são pra ser desbloqueadas** |
| marcos de nível | 0 | se abrem sozinhos ao subir |

O marco de nível não vale XP, e há teste: valer daria XP por ter XP, e o nível
subiria sozinho até o fim da lista — um laço com cara de recompensa.

**As impossíveis existem de propósito.** *Zerou o Odeon* pede as 17.498 obras.
Um acervo desse tamanho precisa de fundo do poço visível; sem ele, "difícil" vira
o teto e a lista acaba.

**E o outro lado da fita está lá.** *Devolveu zoado* — deixar 10 fitas no meio pra
outra pessoa rebobinar — é conquista, com tag. Não é castigo: é fato sobre pessoa
real, que foi o que a R30 (§46) construiu, e a tag é escolha de quem a tem.

### A curva do nível

Triangular: o nível `n` começa em `50·n·(n−1)`. Linear faria o número virar uma
segunda contagem de filmes, dizendo o que "127 obras" já dizia; exponencial
deixaria metade dos níveis inalcançáveis, porque a lista **tem fundo** — o XP
máximo fica perto de 8.000. Há teste que fecha a curva com a sua inversa nos dois
sentidos: se elas divergirem, a barra do perfil enche antes ou depois de o nível
virar, e o número passa a mentir sobre o próprio progresso.

### O perfil, e onde a comparação mora

Título e tags são **chaves de conquista**, não texto livre, e quem valida é o
código — o banco não conhece a lista e não deve conhecer. Um título não
desbloqueado devolve **403**, e não é descartado em silêncio: descartar mostraria
sucesso na tela e deixaria o perfil diferente do que a pessoa mandou.

E a tela nunca oferece o que a validação vai recusar: o servidor manda a lista
dos títulos e tags já abertos. Levar 403 escolhendo de um menu que o produto
mostrou seria o produto mentindo pra si mesmo.

A **bio** é a exceção declarada — foi pedida junto ("as duas coisas"), e ela
convive com as tags de propósito: as tags dizem o que você fez, a bio diz o que
você quer dizer. O risco conhecido, a bio roubar a atenção do que foi
conquistado, é resolvido no tamanho: 140 caracteres, uma linha.

**A comparação com os amigos mora dentro do perfil.** O §40 separou o placar "pra
a decisão ser reversível", e o efeito foi ele ficar escondido numa aba que
ninguém abria. A reversibilidade era real — apagar o placar custou um arquivo e
uma linha, exatamente como previsto —, mas o preço foi a feature não existir. A
comparação fica onde alguém vai olhar.

### As sagas, e a dívida que elas pagam

O `IDEIAS.md` §7 registrava: *"sagas de filme não existem como dado.
`belongs_to_collection` do TMDB não é buscado. Pré-requisito das conquistas de
saga."* Os 007, Alien, De Volta para o Futuro existiam como **pasta no disco**.

A dívida era de dados, não de schema: `collection.kind` aceita `'franchise'`
desde a migração original. **O modelo de grafo do §1 previu a saga antes de
alguém precisar dela**, e é a segunda vez que essa aposta paga — a primeira foi a
ordem alternativa de exibição. Nenhuma tabela nasceu; um job preencheu.

E ele reusa a chamada do §38: `belongs_to_collection` vem na **mesma resposta**
que a ficha de produção. Dois módulos porque são dois jobs com retomadas
diferentes; uma chamada por filme porque pedir duas vezes seria pagar dobrado
pela mesma linha.

| | |
|---|---|
| filmes consultados | 548 |
| entraram numa saga | **315** |
| sagas distintas | **133** |
| falhas | **0** |
| maiores | James Bond (18) · Sexta-Feira 13 (10) · Harry Potter (8) |

**O falso negativo é assumido:** filme sem saga continua elegível e será
perguntado de novo na próxima rodada. Marcá-lo exigiria schema pra guardar
ausência, que é o que o §38 recusou pelo mesmo motivo.

### Verificação

| | |
|---|---|
| migração | 0031 aplicada (em segunda tentativa — ver abaixo) |
| retroativo | 2 conquistas abriram sozinhas, sem backfill: *A primeira fita*, *Não está sozinho* |
| XP · nível | 48 XP, nível 1, `2 de 72` conquistas |
| título não desbloqueado | **403**, com a frase |
| tag não desbloqueada | **403**, nomeando a tag |
| bio livre | 200 |
| sagas | **133** criadas de 548 filmes, 315 filmes ligados, 0 falhas |
| placar | sam e rudney, ordenados por XP, com a própria linha marcada |
| testes · typecheck | **217 passam** (7 novos) · `tsc --noEmit` limpo |
| o aviso | **removido**, junto com `placar.rs` e `Placar.tsx` |

### A armadilha do `sqlx::migrate!` mordeu de novo

Segunda vez em duas fases. O container aplicou uma versão da 0031 anterior à
edição, e a seguinte falhou com *"migration 31 was previously applied but has
been modified"* — o servidor não sobe mais até alguém intervir.

Já está no `CLAUDE.md`, já está no §46, e mordeu de novo. **A lição não é
lembrar melhor**: é que editar um `.sql` já aplicado é uma operação sem rede, e o
`touch src/main.rs` tem que vir *antes* de qualquer reinício, não depois de
notar o estrago.

## 49. R33 — a rede social, e as duas podas por privacidade que ela desfaz

Duas seções deste documento chamaram de vazamento o que a visão chama de
feature. O §41 escreveu que o mural conta *"o que terminou, não o que abriu"*,
porque *"anunciar cada coisa que se provou e abandonou é vigilância com cara de
recurso social"*. O §42 fechou a rota que diz quem está assistindo o quê.

A decisão 2.2 do `IDEIAS.md` é explícita e contrária: **amigo vê o que você está
assistindo agora, o que largou no meio, o que terminou, suas notas. Sem chave de
privacidade.**

**Nenhuma das duas seções era burra**, e vale dizer por quê: as duas foram
escritas quando o escopo era um "círculo" que podia ter um convidado dentro, e
ali a preocupação fazia sentido — você não escolhia quem entrava. Com **amizade
que se aceita** (R28, §44), o aceite *é* o consentimento: você só aparece pra
quem deixou entrar. A poda não foi revertida por capricho; ela perdeu a premissa.

### O que sobreviveu da poda, e é o argumento que ninguém tinha lido direito

O §41 tinha **dois** motivos, e só um caiu. O que caiu foi a privacidade. O que
fica é o volume: um feed sobre `play_event` cru seria um log — 128 linhas dizendo
*"sam abriu Drive"* dezoito vezes, e ruído ensina a não olhar (§24).

Então as fontes novas não são o log cru:

| fonte | o que a impede de virar log |
|---|---|
| `assistindo` | **uma linha por pessoa** — o que está rodando agora, não o histórico |
| `largou` | não terminada, **entre 5% e 85%**, e parada há **mais de um dia** |
| `postou` | é digitado por gente; não tem volume automático |

A terceira condição do `largou` é a que separa "abandonou" de "foi fazer café".
Conferido no acervo: *Ghost in the Shell* a 71% e *Drive* a 61% estão na janela
de fração, e **não aparecem** — pararam há 12h e 13h. A regra está funcionando
justamente quando não mostra nada.

### A presença não vem do transcode, e isso é uma correção

O §42 fechou `/api/transcode/sessions` chamando-a de vazamento. Ela continua
fechada — mas o motivo mudou: ela é o **pior** dos dois sinais disponíveis.

Só enxerga quem está transcodificando, e o §3 decidiu que aqui o caso comum é
**Direct Play**. Uma lista de presença construída sobre ela diria que ninguém
está vendo nada na maior parte do tempo — teria sido uma feature quebrada e
ninguém saberia dizer por quê.

Os dois sinais honestos já estavam no banco desde sempre:

| pergunta | fonte | corte |
|---|---|---|
| está online? | `auth_session.last_seen_at` | 5 minutos |
| está assistindo? | `playback_state.updated_at` | **90 segundos**, o mesmo da locadora (§35) |

Os cortes são diferentes de propósito, e há teste pros dois. `last_seen_at` é
tocado por requisição, e quem está lendo a ficha de um filme passa minutos sem
pedir nada — um corte de 90s ali faria a lista piscar.

### Três tabelas, e é a primeira vez em muito tempo

As cinco fases anteriores previram peça de schema e não criaram nenhuma (§38,
§41, §48). Esta cria três, e a diferença é simples: feed, XP e conquista são
**leituras** de fatos que já existiam; post, comentário e mensagem são **fatos
novos**. Ninguém deriva um texto que uma pessoa escreveu.

### O comentário serve os dois alvos, e é uma tabela só

Decidido: comentário existe **no post e na review** (`IDEIAS.md` §6.2, que estava
em aberto). Post sem comentário é diário, não rede social; e a review foi pedida
com *"as pessoas podem comentar"* explícito.

Duas tabelas quase idênticas seriam duas telas, duas rotas e duas chances de
divergirem sobre o que é um comentário. Uma tabela com **alvo polimórfico** e
CHECK garantindo exatamente um é o mesmo padrão que `emprestimo` usa desde a 0021
— e pelo mesmo motivo: as duas pontas mantêm chave estrangeira de verdade.

A review é apontada pelo par `(quem, qual filme)`, que é a chave primária de
`avaliacao`. Inventar um id só pra ser apontado trocaria a identidade natural por
uma sintética.

### A única restrição da fase inteira

**Mensagem direta só entre amigos.** Não é privacidade sobre o que você assiste —
a decisão 2.2 abriu isso. É que mensagem de estranho é o mecanismo pelo qual toda
rede social vira desagradável, e a amizade aqui já tem aceite: quem quer falar
com você pede amizade primeiro, que é uma tela e um clique.

E o evento do barramento leva **só o aviso, não o texto**: o `EventSource` é
aberto a todos os aparelhos autenticados, então mandar o conteúdo entregaria a
conversa a quem não é dela. Cada cliente descarta o que não é seu pelo `para` —
o mesmo padrão do `ProgrammeStarting` (§25).

### A aba subiu de nível

*"Uma aba separada, que talvez venha a ser algo separado do Odeon."* Ela saiu de
dentro de "experimentação" e virou primeiro nível, com três salas: **mural**
(feed + caixa de post + presença), **conversas** e **gente** (amigos, pedidos e
busca). É o que a deixa pronta pra um dia sair daqui sem arrastar a locadora
junto.

### Verificação

| | |
|---|---|
| migração | 0032 aplicada **de primeira** — o `touch src/main.rs` veio antes do restart |
| post · comentário | criados por duas contas diferentes, o comentário aparece embutido no feed |
| alvo duplo | **400**: *"comente num post ou numa review, exatamente um"* |
| presença | rudney como amigo, sam como eu, ninguém assistindo — e a luz é o que diz "online" |
| mensagem | entregue; a conversa do rudney mostra 1 não lida |
| mensagem a não-amigo | **403**: *"vocês precisam ser amigos pra conversar"* |
| busca | `?q=rud` → rudney, com a relação junto |
| `largou` | 0 hoje, e **corretamente**: as duas candidatas pararam há 12h e 13h |
| telas | mural, gente e conversas conferidas por screenshot |
| testes · typecheck | **219 passam** (2 novos) · `tsc --noEmit` limpo |
| dados de teste | post, comentário, mensagem, a conta `r33teste` e as sessões **apagados** |

## 50. R34 — a revista da semana, e o LLM que só costura

O §30 entregou um **índice**: cartões por diretor, elenco, compositor, gênero,
década e país, cada um cruzado com o seu histórico. É simples e é útil — e é uma
enciclopédia, não uma revista.

O pedido é outro: *"um guia dinâmico, que muda de temática por dia ou semana,
faz eventos de um filme ou saga específica para incentivar as pessoas a
assistir, e usa o acervo para ensinar história do cinema. Útil, não decorativo.
Igual para todo mundo, para haver assunto em comum."*

**O índice não morreu.** Ele desceu, e virou a parte de consulta atrás da capa —
que é a diferença entre uma enciclopédia e uma revista: a enciclopédia continua
ali, só não é o que se vê ao abrir.

### Igual pra todo mundo, e por isso sem tabela

O tema e o evento são `md5(semana || eixo)` sobre o acervo, com a **mesma
semente semanal da locadora** (§36) — e portanto viram na mesma segunda-feira.
Duas visitas na mesma semana veem o mesmo tema; duas pessoas veem o mesmo tema;
e não há nada pra sincronizar nem pra expirar.

Isso é o §2.4 do `IDEIAS.md` virando código: **o guia é coletivo de propósito**,
porque é o que dá assunto em comum. Os desafios (fase 8) são o oposto —
sorteados por pessoa.

Terceira vez que o truque paga: emissora (§25), vitrine (§36), guia.

**Cinco eixos, e nenhum inventado pra esta fase**: gênero e década do M2, país do
§38, diretor do M1, **saga da R32** — a dívida que a fase 5 pagou já rendendo.
Medido: 27 gêneros, 33 países, 66 diretores com 3+ filmes e 44 sagas com 3+. Há
material pra um ano sem repetir, e há teste que confere que os cinco eixos saem
ao longo de 52 semanas — um sorteio que caísse sempre no mesmo faria a revista
ter um tema só.

### O LLM entra, e a arquitetura é a ressalva

O §18 é o pilar mais citado deste documento, e foi usado duas vezes pra recusar
geração de texto — trivia (§32) e retrospectiva (§40). **As duas recusas
continuam de pé:** fato sobre filme, nunca. Trivia inventada sobre um filme que
alguém ama continua sendo pior que nenhuma.

O que a decisão 2.3 abriu é outra coisa — **conteúdo editorial** —, com uma
ressalva que é o desenho inteiro:

> *"O sistema manda os fatos, o LLM escreve a costura."*

A lista de filmes, anos e diretores sai do **banco**. O modelo recebe essa lista
pronta e redige em volta. Ele **nunca** é perguntado *"quais filmes de terror
existem?"*, porque a resposta a essa pergunta é exatamente o que ele inventaria
com confiança. O `SISTEMA` repete a proibição em três linhas, mas a garantia não
é o prompt: é o fato de ele não ter de onde buscar.

E o que sai leva **selo** — o nome do modelo, na tela, como a curiosidade da
Wikipédia leva o crédito (§32). Quem lê tem direito de saber que aquele
parágrafo não foi escrito por gente.

### Sem chave é um estado normal, e ele foi exercitado antes da chave existir

A integração lê `GROQ_API_KEY` do ambiente, e sem ela a capa mostra o tema e os
filmes (que são fato do banco) e **omite o ensaio** — o §18 e o §24 na mesma
decisão: não inventar, e não escrever "em breve" no lugar. O ensaio é gerado
**fora da requisição**, então a capa nunca espera por um modelo.

A fase foi entregue nesse estado, com o buraco declarado. A chave chegou depois,
e o resto desta seção é o que aconteceu quando ela chegou.

### O primeiro ensaio foi honesto e inútil

Com a chave posta, o texto saiu na primeira visita. A auditoria da ressalva
passou limpa — **zero filmes inventados, zero anos inventados, zero diretores
inventados**. E o texto era isto:

> *"Romance é o tema da semana. Temos alguns filmes que se encaixam nesse gênero,
> como Será Que?, dirigido por Michael Dowse (…) Esses filmes estão disponíveis
> em nossa locadora para alugar."*

Nada falso, e nada aprendido. Ele abre com a frase que o próprio prompt proibia e
fecha com enchimento de catálogo. O `IDEIAS.md` §3.1 pede o contrário: *"usa o
acervo para ensinar história do cinema. **Útil, não decorativo**."*

**A culpa era dos fatos, não do modelo.** Ele recebia título, ano e diretor — e
com três colunas não há o que dizer além de listar. Um modelo só escreve sobre o
que recebe.

### O conserto foi dar material, não pedir esforço

Entraram no prompt o **país** de cada filme e, principalmente, uma seção de
**ligações medidas no acervo**: o intervalo de anos, as décadas que concentram,
o país que mais aparece, quantos a pessoa já viu, e **quantos filmes o tema tem
ao todo** — o mesmo denominador da placa que diz "3 de 113" (§14).

Nada disso é opinião: são todos `SELECT`s. O que o modelo faz continua sendo só a
costura. E o `SISTEMA` ganhou uma lista do que é enchimento, com as frases exatas
que a primeira versão produziu.

O segundo texto abre por uma observação sobre o conjunto:

> *"O que estes filmes têm em comum é a variedade de décadas em que foram
> produzidos, desde os anos 80 até 2013. Aladdin, de 1992, é um clássico da
> animação romântica, enquanto Sim Senhor, de 2008, traz um toque de comédia ao
> gênero."*

Auditado: **quatro anos citados, os quatro da lista**; três títulos, os três da
lista; nenhum nome próprio fora dela.

Ainda não é ótimo — o fecho *"mostram a diversidade do tema"* é enchimento que
sobreviveu. Mas mudou de natureza: é específico, é verificável, e ensina alguma
coisa sobre o recorte. A diferença entre as duas versões não foi um prompt mais
insistente; foi **ter o que dizer**.

### O evento

Um filme ou uma saga em cartaz na semana — saga primeiro, porque foi o pedido e
porque uma saga dá o que fazer a semana inteira enquanto um filme dá duas horas.

Participar é **terminar durante a janela**: o mesmo sinal do §8f que a curadoria,
a locadora, o mural e as conquistas já usam. Uma sexta definição de "participou"
seria uma sexta chance de discordarem.

Ele amarra a revista com o resto da sequência: dá **XP e quatro conquistas
novas** (fase 5), e quem participou aparece pra todo mundo na capa — que é o
ponto de ele ser coletivo.

**Duas tabelas, e as duas se justificam por não serem deriváveis.** O `ensaio` é
cache de uma função cara; apagá-lo só custa gerar de novo. A
`evento_participacao` congela o que a janela não deixa recuperar: a semana passa,
o tema vira, e *"terminou enquanto estava em cartaz"* deixa de ser recuperável.
É a mesma razão pela qual `emprestimo.devolvido_como` é congelado (§35).

### O defeito que a verificação encontrou

`avaliar` (conquistas) rodava **antes** de `talvez_participou`. Compilava,
passava nos testes e funcionava — só que a conquista *Esteve lá* abria na **ação
seguinte**: a pessoa terminava o filme do evento e a medalha aparecia amanhã,
quando clicasse em outra coisa.

Trocar a ordem é uma linha. Encontrar exigia terminar um filme de verdade e
olhar a resposta. **Conferido depois da troca:** *A primeira fita* e *Esteve lá*
chegam no mesmo gesto.

### Verificação

| | |
|---|---|
| migração | 0033 aplicada de primeira |
| a capa | eixo `genero`, tema **Romance**, 8 filmes com ano e diretor |
| o evento | saga **Dr. Dolittle: Coleção**, 2 obras |
| participação | terminar uma obra da saga registrou a linha da semana |
| a ordem | conquistas *A primeira fita* + **Esteve lá** no mesmo gesto, depois do conserto |
| coletivo | o participante aparece na capa de outra pessoa |
| XP | 52 = 20 de conquista + 32 de atividade, com o evento valendo 20 |
| ensaio, sem chave | **ausente**, e a seção some — a tela não inventa nem escreve "em breve" |
| ensaio, com chave | escrito na primeira visita, com selo `llama-3.3-70b-versatile` |
| **a ressalva** | auditado nas duas versões: **0 filmes, 0 anos e 0 diretores inventados** |
| o índice | intacto, abaixo da capa |
| testes · typecheck | **222 passam** (5 novos) · `tsc --noEmit` limpo |
| dados de teste | conta `r34teste`, participação e sessões **apagadas** |

## 51. R35 — os desafios, e o fim da lista

O último dos onze itens, e o único que nunca tinha sido construído.

> *"Tarefas com prazo, que dão experiência. Mais simples que os temas do guia, e
> sorteadas para cada pessoa — não são iguais pra todos. A cadência é escolhida
> pela pessoa, entre algumas opções definidas."*

### O oposto do guia, e o §2.4 no schema

A decisão 2.4 separa as duas coisas numa tabela de duas colunas: **coletivo**
(guia da semana, eventos) contra **individual** (desafios, XP, conquistas). Isso
aparece direto na modelagem:

| | guia (§50) | desafio |
|---|---|---|
| quem vê | todo mundo, o mesmo | cada um o seu |
| onde mora | derivado, sem tabela | tabela |
| por quê | recalculável de `md5(semana)` | a janela de cada um começa num instante diferente |

E há um segundo motivo: **"cumpriu dentro do prazo" deixa de ser recuperável
quando o prazo passa.** É a mesma razão da `evento_participacao` (§50) e do
`emprestimo.devolvido_como` (§35) — três vezes o mesmo argumento, e ele continua
sendo o único que faz uma tabela nascer neste projeto.

### Três por janela, e o terceiro é o que justifica a fase

| fatia | XP | o que faz |
|---|---|---|
| **fácil** | 15–20 | dopamina — e não é só assistir: avaliar, alugar, resenhar e rebobinar entram |
| **tema** | 30 | um gênero, uma década, uma fita |
| **empurrão** | 50 | o que você **nunca** viu: um país, um diretor, um gênero inédito |

O terceiro é o único que faz o desafio servir ao **terceiro pilar** (§1). Sem
ele, um sistema de tarefas sorteadas do seu próprio gosto só reforça o gosto — e
aí ele é entretenimento, não curadoria.

**A semente é da pessoa, da janela e da cadência.** É o que faz o desafio ser
individual onde o guia é coletivo, com o mesmo mecanismo de sorteio
determinístico.

### Falhar não custa nada, e isso é uma decisão

A janela fecha, o desafio some, outro é sorteado. Sem perda de XP, sem sequência
quebrada, sem aviso.

A alternativa considerada — uma sequência que zera ao falhar — é o motor mais
forte que esse tipo de sistema tem, e é exatamente por isso que foi recusada.
**Este projeto tem uma punição só, e ela é social**: a fita mal devolvida (§46).
Ela funciona porque é entre pessoas e porque o atrito é a graça. Punir alguém por
não ter assistido um filme é o placar do §40 voltando com outra roupa.

A tela segue a decisão: sem barra de progresso, sem contador de aproveitamento,
sem vermelho. Uma tela que sugere cobrança cobra, mesmo que o código não cobre.

### Sem job, de novo

A geração acontece **na leitura**, e é idempotente pelo `UNIQUE` da tabela: abrir
a tela duas vezes na mesma janela não sorteia dois conjuntos. Um processo de
fundo pra criar três linhas quando alguém abre uma tela seria uma peça a mais
pra quebrar — é a mesma decisão que pôs a devolução automática da locadora na
leitura (§35) em vez de num daemon.

### O defeito que só existe às segundas-feiras

Todas as cadências são ancoradas na **segunda-feira local**, como a vitrine
(§36) e o guia (§50). A âncora está certa: sem ela, a janela de cada pessoa
flutuaria a partir do dia em que ela escolheu a cadência.

Mas numa segunda-feira a janela **diária e a semanal começam no mesmo
instante**. E como a identidade da linha era `(user_id, comeca_em, chave)`,
trocar de cadência naquele dia não gerava nada — o `ON CONFLICT DO NOTHING` via
as linhas de antes e desistia.

O sintoma é discreto, e por isso pior: os desafios apareciam, o botão dizia
"todo dia", e o prazo continuava sendo o de domingo. **Ninguém veria isso como
defeito — veria como o produto ignorando a escolha.**

A cadência entrou na chave (0035). Duas janelas que começam juntas e duram
tempos diferentes são duas janelas.

E ele só existia **um dia por semana**. Encontrado porque a verificação caiu
numa segunda-feira e eu troquei a cadência pra ver o que acontecia — não porque
algum teste soubesse perguntar isso.

### Verificação

| | |
|---|---|
| migrações | 0034 e 0035 aplicadas |
| sorteio | três por pessoa, **diferentes entre as duas contas** |
| idempotência | duas chamadas na mesma janela → 6 linhas para 2 pessoas, não 12 |
| cumprimento | terminar um filme de Crime fechou o desafio **e** abriu *Topou* no mesmo gesto |
| XP | 62 = 20 de conquista + 12 de atividade + **30 da linha do desafio** |
| cadência | as três trocam; a diária vence amanhã, a semanal na segunda |
| o defeito | corrigido e conferido: trocar pra diária numa segunda passou a gerar janela nova |
| a janela | 5 testes travam a âncora, a prova dentro do prazo e a semente |
| testes · typecheck | **229 passam** (11 novos) · `tsc --noEmit` limpo |
| dados de teste | conta `r35teste`, desafios, perfis e sessões **apagados** |

### O fim da sequência

As oito fases do `IDEIAS.md` §5 estão feitas. Os onze itens das anotações
originais têm resposta, e as cinco perguntas em aberto da §6 foram respondidas —
todas por quem decide, nenhuma por quem programa.

O que fica em aberto está registrado no lugar certo e não disfarçado: **os
clientes Kotlin pararam no M2** e não conhecem nada do que foi construído hoje;
e **não há CI** — 229 testes que ninguém roda automaticamente.

*(A chave do Groq, que faltava quando isto foi escrito, chegou logo depois. O que
aconteceu quando ela chegou está no §50, e a lição vale mais que a feature: um
modelo que recebe três colunas escreve sobre três colunas.)*

## 52. R36 — a barra de cima, e o estacionamento que virou endereço

Esta seção não veio das onze anotações. Veio de *"nosso menu superior tá bem
feio"*, com autorização explícita pra ser experimental — e com uma frase que
resolveu metade do problema antes de eu escrever qualquer coisa:

> *"as coisas experimentais não precisam ficar pra sempre no experimental; se
> terminamos já tá de boa pra tirar e colocar no seu lugar."*

### O defeito era de arquitetura, não de estilo

A barra tinha **nove entradas em fileira**, mais **quatro salas escondidas**
dentro de uma delas chamada "experimentação". E as nove misturavam três coisas
diferentes com o mesmo peso visual:

| tipo | entradas |
|---|---|
| acervo | biblioteca, coleções |
| produto | para você, experimentação, mural, ao vivo |
| **manutenção** | revisão, pastas, admin |

É **exatamente** o defeito que o §12 corrigiu quando tirou as operações de
servidor daqui — *"misturadas, elas competiam com as abas, e a mais gritante da
tela era `identificar`"*. Seis fases depois ele tinha voltado por outro caminho:
ninguém acrescentou uma operação à barra, mas acrescentou três telas de
manutenção, uma por vez, e cada uma parecia inofensiva sozinha.

**E "experimentação" era um estacionamento.** A palavra não descreve nada; ela
existia porque a locadora, o guia, a retrospectiva e o perfil estavam sendo
construídos. Estão prontos há sete fases — e uma feature pronta atrás de uma
palavra que não a descreve é uma feature que ninguém acha.

### O que ficou

Dois lados, e a divisão é a do §12 aplicada de novo: **navegação de um lado,
ferramenta do outro.**

À esquerda, **sete entradas, todas do mesmo tipo** — lugares do acervo: para
você · biblioteca · coleções · locadora · guia · ao vivo · mural. À direita, o
que não é acervo: a manutenção atrás de um ícone de controles, e você atrás do
seu próprio nome.

A retrospectiva foi pro **perfil**, que era o destino que o `IDEIAS.md` §4 já
tinha previsto pra ela: *"pode sobreviver como tela de perfil"*. Faz mais
sentido lá — ela descreve quem você é, que é literalmente o assunto daquela
tela.

### Os efeitos, e o que cada um responde

Nenhum é enfeite solto. Cada um responde uma pergunta que a barra antiga deixava
a tela responder sozinha:

| efeito | o que ele diz |
|---|---|
| o traço que **desliza** entre as abas | de onde você veio, não só onde está |
| o **holofote** que segue o mouse | onde o dedo está, numa fileira de sete alvos pequenos |
| a barra que **condensa** ao rolar | você saiu do topo; o conteúdo é que importa agora |
| o **anel** em volta do seu nome | quanto falta pro próximo nível, sem abrir o perfil |
| a marca que **pulsa** | tem trabalho rodando no servidor |
| o conteúdo que **entra subindo** | a aba trocou; não foi falha de carregamento |

**O traço mora fora dos botões**, e isso é a diferença inteira: uma borda por
botão não desliza de um pro outro — ela aparece num e some do outro, que é o que
a barra fazia antes. Posição e largura vêm do DOM, medidas: os rótulos têm
tamanhos diferentes e a fonte é do sistema, então a única fonte de verdade sobre
onde a aba está é a própria aba.

**O anel é a fase 5 aparecendo onde ela é útil.** Um `conic-gradient` até a
fatia do nível, e ele entra girando de zero na carga — um arco que já nasce
cheio não é lido como progresso, é lido como enfeite.

Tudo isso desliga em `prefers-reduced-motion`. Alma não pode custar enjoo a quem
pediu pra não ter movimento.

### Duas coisas que o screenshot corrigiu

**O `⚙` do sistema é emoji, e emoji vem colorido** — um ícone azul e vermelho no
meio de uma barra âmbar e cinza. Redesenhei em SVG como engrenagem de quatro
dentes, e a 16px ele lia como **estrela**. A terceira versão são três trilhos com
um botão cada: diz "ajustes" em qualquer tamanho, e os botões deslizam no hover,
que é o gesto do próprio ícone.

**O vidro deixava os pôsteres atravessarem.** Condensada, a barra passa por cima
da vitrine da locadora — oito capas — e a 92% de opacidade o rótulo da aba
competia com uma delas. Foi pra 97% com mais desfoque. Nenhum dos dois apareceu
lendo o código.

### E um defeito que a transição de aba causou

A entrada do conteúdo subia com `transform: translateY(8px)` e
`animation-fill-mode: both`. Passou no typecheck, nas sete abas do teste de
fumaça e em quatro screenshots. **E quebrou todo overlay de dentro do `main`** —
a caixa da locadora voando pro centro, o menu de DVD, a tela da fita.

O motivo é uma linha do CSS que quase nunca importa:

> **qualquer `transform` diferente de `none` faz o elemento virar bloco de
> contenção para os `position: fixed` descendentes.**

E `fill-mode: both` deixa a animação preenchendo pra sempre. O valor final é a
matriz identidade — `matrix(1,0,0,1,0,0)` —, que não é `none`. O `<main>` virou
a referência de posicionamento de tudo que era fixo dentro dele, **em
definitivo**, e ninguém notaria olhando o CSS: o valor é visualmente nulo.

Medido no navegador depois do relato: `.mao-fundo`, que é `fixed; inset: 0`,
devolvia `{y: -277, h: 3907}` — o tamanho da página inteira — e a caixa pousava
em `y = 1326` numa viewport de 814. Ela ia pro foco e saía de vista, que foi
exatamente a frase do relato.

O conserto é `position: relative` + `top`, que dá o mesmo movimento e **não**
cria bloco de contenção pra `fixed`. Custa um reflow de 8px num elemento; em
troca é seguro por construção em vez de seguro por 280 milissegundos.

**A lição é sobre o teste, não sobre o CSS.** Eu cliquei as sete abas e li o
texto de cada tela — e as sete estavam certas. O que quebrou foi um overlay que
só existe depois de um segundo clique, dentro de uma delas. Um teste de fumaça
que só confirma que a tela abriu não cobre o que a tela abre.

### Verificação

| | |
|---|---|
| as sete abas | clicadas uma a uma: **todas abrem, nenhum erro**, e o traço acompanha |
| a caixa da locadora | voa pro centro e **fica**: `.mao-fundo` casa com a viewport, a caixa pousa em `y = 57` de 814 |
| o menu de DVD | `[0, 0, 1428, 914]` contra viewport `[1440, 914]` — o outro overlay que o defeito derrubava |
| perfil | atrás do menu do usuário, com a retrospectiva dentro |
| gavetas | manutenção e você abrem, fecham no Escape e ao clicar fora |
| estar numa tela de dentro | acende a borda do botão — sem isso a barra ficava sem nada marcado |
| condensada | conferida por screenshot em cima da vitrine, que é o pior fundo que existe |
| typecheck | `tsc --noEmit` limpo |

## 53. R37 — a auditoria de permissão, e o buraco que tinha gente dentro

Esta seção veio de uma linha de quem decide, escrita em caixa alta:

> *"Verificar se usuário normal tem acesso a qualquer configuração do servidor,
> **PRINCIPALMENTE usuário normal não pode apagar nem modificar nada**."*

### O que a auditoria encontrou

Há um `require_auth` global desde o §9b, e ele faz o que promete: valida a
sessão. **Ele não olha papel** — nunca prometeu isso. Quem separa `admin` de
`user` é o extrator `AdminUser`, aplicado handler a handler.

Contadas: **25 rotas de escrita com `AdminUser` e 51 sem**. A maioria das 51 é
legítima — progresso, nota, empréstimo, post, comentário, mensagem, amizade,
perfil, desafio são todas do próprio usuário. Sobraram estas, sondadas com o
token do `rudney` (`role = user`) e UUIDs inexistentes, pra não tocar em nada:

| rota | antes | devia ser |
|---|---|---|
| `PATCH /api/collections/{id}` | 404 | 403 |
| `DELETE /api/collections/{id}/items/{work}` | **200** | 403 |
| `PUT /api/collections/{id}/order` | 200 | 403 |
| `POST /api/works/{id}/tags` | 500 | 403 |
| `DELETE /api/works/{id}/tags/{tag}` | **200** | 403 |
| `POST /api/works/{id}/relations` | 422 | 403 |
| `DELETE /api/works/{id}/relations/…` | 200 | 403 |

404, 200 e 422 significam a mesma coisa: **a autorização deixou passar** e o
pedido chegou no handler. Os dois 200 são deleções que **executaram** — não
apagaram nada só porque o alvo não existia.

**E o buraco tinha gente dentro.** No meio da auditoria apareceu uma terceira
conta neste servidor — `gabriel`, `role = user`, criada às 15:54, vinte minutos
antes do relato. Deixou de ser hipotético.

### A regra: origem, não papel — e não é a mesma pra tudo

Coleção é **duas coisas diferentes** com a mesma tabela:

| | quem pode | por quê |
|---|---|---|
| `origin = 'provider'` (as **709** deste servidor) | ninguém edita à mão | é acervo: série, temporada e as 133 sagas da R32 |
| `origin = 'manual'` ("suas ordens", §17) | qualquer morador | a ordem Machete é uma **opinião**, e opinião é de quem tem |

Exigir administrador pra criar uma ordem de exibição mataria a feature. Então a
linha divisória é a **origem**, não o papel — e `create_collection` já gravava
`'manual'` fixo desde o §17, então ninguém cria uma `provider` por essa porta.

`delete_collection` **já conferia isso**, e conferia bem. As outras quatro rotas
que mexem numa coleção — renomear, acrescentar, tirar e reordenar — não
conferiam nada. A checagem virou uma função (`so_manual`) usada pelas cinco: a
regra existia num lugar e faltava em quatro, que é como uma regra some.

**Tag e relação são outra história**, e essas viraram de administrador. A
diferença é de dono: uma ordem de exibição é sua; uma tag na obra e um "corte do
diretor de" mudam o que **todo mundo** vê — e a curadoria (§8f) e o guia leem
`work_tag` como verdade sobre o acervo.

### A tela parou de oferecer o que o servidor recusa

O botão `✎ editar` da ficha abria "edição do grafo" — tag, coleção e relação —
**para qualquer conta**. Agora ele não nasce pra quem não é administrador.

Deixar o botão aparecendo pra quem vai levar 403 é o produto mentindo pra si
mesmo. É a mesma regra que o perfil (§48) já seguia ao só listar os títulos que
a pessoa desbloqueou: *a tela nunca oferece o que a validação vai recusar*.

### Verificação

Sondas com os dois papéis, e o contraste é a prova:

| rota | rudney | sam (admin) |
|---|---|---|
| `PATCH` / `DELETE` a coleção do Harry Potter | **403** | 403 · *provider é intocável pros dois* |
| `DELETE` item da coleção · `PUT` ordem | **403** | 403 |
| `POST` / `DELETE` tag | **403** | passa |
| `POST` / `DELETE` relação | **403** | passa |
| `POST` criar coleção ("suas ordens") | 200 | 200 · e nasce `manual` |

Acervo conferido depois: **709 coleções, 8.725 itens, 21.923 tags** — os mesmos
números de antes. As duas coleções `__sonda__` criadas no teste foram apagadas.

### O que isto NÃO fecha, e fica dito

**Coleção `manual` não tem dono.** Não há coluna de autor, então uma ordem de
exibição criada pelo `rudney` pode ser apagada pelo `gabriel`. Hoje isso é
teórico — existem **zero** coleções `manual` no servidor —, e fechar exige uma
migração e uma decisão sobre o que fazer com as órfãs. É planejamento, não
conserto de emergência.

**E `attach_tag` devolve 500 pra obra inexistente**, onde devia ser 404. É a
mesma família do §8b — errar com o código errado é a versão barulhenta de errar
em silêncio. Não é buraco de segurança; é aspereza, e fica anotada.

## 54. R38 — as capas das sagas, e a chamada que faltou

Duas linhas da lista de quem decide, e elas são **um defeito só**:

> *"Em guia o cartaz da semana tá quebrado"* · *"Na real diversas capas no guia
> estão quebradas"*

### O que estava medido

| | |
|---|---|
| sagas com pôster **remoto** (`/mv0MySTq….jpg`) | **131** |
| séries com pôster **local** (UUID no cache) | **113** |
| sagas com cor dominante | **0 de 133** |

As duas colunas guardam a mesma coisa e não guardam a mesma coisa. O front
prefixa `/artwork/`, o `ServeDir` procura o arquivo no cache e responde 404 — a
moldura fica vazia, e o "cartaz da semana" é justamente onde uma saga aparece
sozinha e grande (`revista.rs`, `evento_da_semana`).

### A causa é da R32, e ela tem nome

O `metadata/saga.rs` gravava o `poster_path` cru do TMDB no `artwork` da
coleção. O pipeline de série chama `artwork::fetch` desde o M1 — baixa, guarda
no cache e devolve o caminho servível, com a cor dominante de brinde. Aqui a
chamada faltou. Não houve decisão errada: houve uma linha que não foi escrita, e
oito meses de moldura vazia por causa dela.

**Guardar caminho quebrado é pior que guardar nada.** É o §18 pelo avesso: o
campo tem cara de metadado válido, a tela acredita nele e desenha a moldura. Uma
coluna vazia teria sumido sozinha (§24).

### O conserto tem duas metades, e as duas moram no job

Quem decide escolheu **a varredura dentro do próprio job de sagas**, e não uma
rota nova nem um script de uma vez só. A razão é a mesma que o §3.5 do
`IDEIAS-2.md` já tinha dado pro botão que vem a seguir: *"achei filmes novos"* e
*"as capas deles apareceram"* devem ser um gesto só. Uma rota a mais seria uma
sétima porta pra cuidar; um script descartável não serviria pra saga nova que
quebrasse depois.

**A varredura não custa uma chamada de API.** O caminho remoto que a R32 deixou
no banco é o endereço da arte — o reparo só precisa remontar a URL e baixar. E
o alvo da consulta é o **próprio estado errado** (`artwork->>'poster' LIKE
'/%'`), então ela é retomável sem coluna de controle, pelo mesmo truque do resto
do módulo: o que já foi consertado deixa de aparecer.

O reparo roda **antes** da busca, porque não depende de rede lenta de API e
porque a saga criada logo adiante já nasce com a arte no lugar — não há trabalho
repetido. O `INSERT` da coleção não escreve mais arte nenhuma; quem escreve no
`artwork` é o `baixar_arte`, e só depois do download.

**Quando o download falha, o caminho remoto fica gravado** — de propósito. Ele é
o único registro de onde a arte mora, e é o que faz a saga voltar a ser alvo na
rodada seguinte. Não gravar nada perderia o endereço e exigiria uma chamada de
API pra reencontrá-lo. O status conta essas como `capas_pendentes`, que é erro
visível e não erro em silêncio (§8b).

### Verificação

Uma rodada completa do job, `POST /api/maintenance/aquecer-sagas`:

| | |
|---|---|
| itens do job | **364** — 131 capas quebradas + 233 filmes sem saga |
| capas baixadas | **260** — 131 pôsteres e 129 backdrops |
| capas pendentes · falhas | **0** · **0** |
| sagas novas nesta rodada | 0 · o acervo não mudou desde a R32 |

Acervo depois, e é o contraste que prova:

| | antes | depois |
|---|---|---|
| sagas com caminho remoto | **131** | **0** |
| sagas com arquivo local | 0 | **131** |
| sagas com cor dominante | 0 | **131** |

A cor veio de graça, no mesmo `artwork::fetch` — 131 sagas que hoje tingem a
própria moldura como toda série já tingia.

Conferência visual em Firefox headless, contando `img.complete &&
naturalWidth === 0`, que é exatamente o 404 na moldura:

| tela | imagens | quebradas |
|---|---|---|
| guia | 222 | **0** |
| coleções | 272 | **0** |

O cartaz da semana desta semana é *Dr. Dolittle: Coleção*, e ele aparece.

**232 testes**, três novos e todos no `saga.rs`: o `INSERT` da saga não escreve
no `artwork`, a varredura mira o caminho remoto, e o arquivo do cache não é
confundido com caminho do TMDB. O primeiro é regressão pura — é o teste que
teria pego isto em 2025.

### O que isto NÃO fecha, e fica dito

**Duas sagas guardam `{"poster": null, "backdrop": null}`** — *The Red Hood
Collection* e *F1 Collection*, que o TMDB não tem arte nenhuma. A chave existe
com valor nulo, o que é herança do `INSERT` antigo. Hoje é inofensivo, porque
nenhuma consulta testa `artwork ? 'poster'` numa `franchise` — só em `series` e
em `work`. Mas é uma chave que responde "sim, tem pôster" pra quem perguntar
assim, e isso é o §18 esperando a vez.

**As 461 temporadas não têm pôster nenhum** — medido de passagem, e não mexido.
Elas nunca tiveram; não é regressão desta rodada nem estava na lista.

**O job continua sem botão.** A rota só é alcançável por `curl`, que é o item 3
do `IDEIAS-2.md` e o próximo da fila.

## 55. R39 — três defeitos pequenos, e um deles não era onde parecia

O item 2 do `IDEIAS-2.md`: a ordem dentro da franquia, o canal da casa que não
abria, e o player que começava curto. Nenhum é grande. Um deles tinha a causa
no lugar errado, e é o que esta seção tem de interessante.

### A ordem dentro de uma franquia

> *"Em Coleções dentro de uma franquia a lista deveria estar ordenado por ano"*

A consulta dos itens era `ORDER BY ci.position NULLS LAST, w.title`. **Medido:**

| coleção | itens | com `position` | com ano |
|---|---|---|---|
| `season` | 8.410 | **8.410** | 8.410 |
| `franchise` | 315 | **0** | 315 |

As sagas do TMDB chegam sem posição, então caíam no alfabético — e é por isso
que *Câmara Secreta* vinha antes de *Pedra Filosofal*. As temporadas têm posição
em toda linha e não sentem a mudança.

A ordem passou a ser `position → ano → título`. **A `position` continua mandando
onde existe**, e isso não é detalhe de implementação: é ela que carrega a ordem
Machete e as ordens manuais, que são **opinião** — e opinião tem precedência
sobre cronologia. O ano só decide onde ninguém opinou.

A regra não precisou olhar `origin`. A distinção que o `IDEIAS-2.md` §3.2
descreve — *"dentro de uma coleção `provider`"* — já está codificada na própria
`position`: quem tem ordem própria tem posição.

### O canal da casa não abria

> *"AO VIVO: Na linha do tempo eu não consigo clicar em cima dos canais odeon"*

O clique chegava. O `onAbrir` procurava o bloco na **grade do IPTV**:

```js
const p = guia?.programas.find((x) => String(x.id) === b.id);
if (p) setDetalhe(p);          // Odeon nunca acha, e some em silêncio
```

Os canais da casa são programados pela emissora (§25) *sem tabela* — o id deles
é `slug:índice` (`odeon-1:7`), que nenhum id numérico de IPTV vai igualar. `p`
vinha `undefined` e o `if` engolia. **É o §8b inteiro**: um clique que não faz
nada não é recurso ausente, é recurso quebrado.

O bloco da casa agora abre o **cartaz da obra**. Ele já carregava `work_id` —
`ProgramaOdeon.work_id` é `Uuid`, não `Option<Uuid>`, porque a emissora programa
a partir do acervo. Um programa de IPTV tem título e horário; um bloco do Odeon
tem uma obra, que é mais.

E a terceira saída deixou de ser o silêncio: bloco sem programa **e** sem obra
agora diz isso na tela, em vez de repetir o defeito por outro caminho.

### O player que começava curto — e a causa que estava em outro arquivo

> *"Alguns filmes estão com o player estranho, começa com um tempo de filme mega
> pequeno e vai aumentando ao longo que vai carregando"*

O `IDEIAS-2.md` §3.4 apontou o mecanismo certo: a sessão HLS é criada com
`-hls_playlist_type event` e sem `#EXT-X-ENDLIST`, então `video.duration` é a
soma dos segmentos prontos e cresce enquanto o ffmpeg trabalha. E propôs que a
barra passasse a usar a duração da obra.

**Ela já usava.** O commit anterior a este documento (515b55c, 02/08) tinha feito
exatamente isso — `total` sai de `work.duration_seconds`, a duração do stream
ficou só marcando até onde dá pra pular, e a timeline até pinta o trecho fora de
alcance. A proposta já estava implementada um dia antes de ser escrita.

O que sobrou foi **quem entrega a obra ao player**. O menu de DVD monta o objeto
à mão, com um comentário que dizia *"manda o mínimo que `Player` usa"*:

```js
{ id, title, year, media_file_id, poster, dominant_color }
```

`duration_seconds` não estava na lista, e o `MenuDoDisco` tinha o número o tempo
todo (`duracao`). Sem ele o player caía no `offset + streamDuration`, que é
justamente o número que cresce. **Por isso "alguns filmes"**: o sintoma não
depende do arquivo, depende da porta — só os filmes abertos pelo menu de DVD, e
esse é o caminho principal da locadora.

O comentário estava errado antes do código: um "mínimo" enumerado à mão envelhece
calado quando quem consome ganha um campo novo.

### Verificação

A franquia do Harry Potter, pela rota da coleção:

| antes | depois |
|---|---|
| *Câmara Secreta* · *Cálice de Fogo* · *Enigma do Príncipe* … | **2001 Pedra Filosofal · 2002 Câmara Secreta · 2004 Prisioneiro de Azkaban · 2005 Cálice de Fogo · 2007 Ordem da Fênix · 2009 Enigma do Príncipe · 2010/2011 Relíquias 1 e 2** |

Uma temporada de série conferida no mesmo caminho continua em 1, 2, 3, 4, 5, 6 —
a `position` mandando, como antes.

Em Firefox headless, na aba "ao vivo": clicar no bloco *Sonic 3: O Filme* da
pista `Odeon 1` abre o cartaz da obra, com arte, elenco e "você sabia". Antes,
nada acontecia.

E o player, aberto pelo caminho da mão — prateleira → caixa → abrir → tocar →
menu → *Tocar*, com ponteiro de verdade no `.tocar` porque `setPointerCapture`
redireciona o `click`:

| | relógio na tela | `video.duration` |
|---|---|---|
| aos 23s | `0:23 / **42:52**` | **44,3 s** |
| aos 43s | `0:43 / **42:52**` | 2.573 s |

A segunda coluna é o defeito ao vivo: a duração do stream **era** de 44 segundos
enquanto o filme tem 42 minutos, e ela quadruplicou de tamanho em vinte segundos.
A primeira não se moveu. `duration_seconds` da obra no banco: **2.572,959 s** —
que é o `42:52` da tela.

**233 testes**, um novo: a ordem da coleção é `position → ano → título`, porque
inverter isso quebraria a ordem Machete em silêncio e nenhuma tela denunciaria.

### O que isto NÃO fecha

**Os outros campos que o menu de DVD não manda.** `series_title`,
`season_number`, `episode_number`, `height`, `video_codec` e `audio_codec`
continuam ausentes — o `MenuDoDisco` não os carrega, e o cabeçalho do player
mostra menos ficha técnica quando o filme vem por essa porta. É cosmético, ao
contrário da duração, e consertá-lo é aumentar a resposta do menu.

## 56. R40 — os aquecimentos ganham porta, e a varredura ganha um fim

> *"Como dou refresh nas coleções para pegar filmes novos?"*

A resposta honesta era: **por `curl`.** A rota `POST /api/maintenance/aquecer-sagas`
existe desde a R32 e nunca teve botão.

### O §27 outra vez, e três em vez de uma

O `IDEIAS-2.md` §3.5 pediu *"botão na aba admin, ao lado dos outros
aquecimentos"*. **Medido: não havia outros.** Os três — `aquecer-trivia` (R14),
`aquecer-producao` (R22) e `aquecer-sagas` (R32) — estavam no mesmo estado, sem
cliente nenhum.

É literalmente o defeito que o §27 corrigiu uma vez: *"sete rotas existiam sem
nenhum cliente, e quatro delas só eram alcançáveis por `curl`"*. Um poder no
backend que nenhuma tela alcança não é um poder do produto — é uma anotação.

Quem decide escolheu fechar a família inteira. A seção **Aquecimentos** nasce com
os três, ao lado da Manutenção e com a mesma grade de cartões.

**E sem ensaio, ao contrário da Manutenção logo abaixo.** A diferença não é de
cuidado, é de natureza: manutenção *reescreve* o acervo — reparse, títulos de
episódio, artwork órfão —, e por isso o §27 pôs o `dry_run` na frente dela. Um
aquecimento **preenche o que está vazio**: pergunta ao provider o que ainda não
foi perguntado e grava onde não havia nada. Não há o que ensaiar.

O progresso não foi reinventado: ele já existia em **Trabalhos**, e cada cartão
mostra o do seu próprio `kind`. Enquanto algo roda, a tela recarrega **só a lista
de trabalhos**, de dois em dois segundos — as outras três chamadas do painel
(contas, aparelhos, diagnóstico) não mudam porque um job andou.

O cartão diz o que o job publica: rodando, é `feitos de total · onde está`; parado,
é `concluiu 03/08 · 364 de 364`. E **"nunca rodou" quando nunca rodou** — não um
`0 de 0` com cara de resultado (§18).

### A varredura chama as sagas — depois da identificação

A segunda metade do §3.5: *"a varredura chamar as sagas no fim, pra 'achei filmes
novos' e 'as sagas deles apareceram' serem um gesto só"*.

**O lugar certo não é depois da varredura, é depois da identificação.** O alvo do
job de saga é filme com `match_state IN ('auto','confirmed')` e id do TMDB —
encadeá-lo logo após o `scan_all` acharia **zero**, porque nenhum arquivo novo foi
identificado ainda. E seria um defeito invisível: o job rodaria, terminaria bem, e
não faria nada.

Então ele entra no fim da corrente que já existia (`?then=match`), depois do
`run_matching` e da publicação do `MatchFinished`. Rodar de novo é barato e sempre
seguro — o alvo é "filme sem franquia", então a segunda passada custa as chamadas
dos avulsos e mais nada, e desde a R38 ela ainda conserta capa de saga que tenha
ficado com caminho remoto.

`Job::start` devolvendo `None` ali é o caso normal de "já há um rodando", e não um
erro: quem apertou o botão à mão ganha a rodada, e o encadeamento não precisa de
uma segunda.

### Verificação

Em Firefox headless, na aba admin: a seção mostra os três cartões com o estado de
cada um lido do histórico — `concluiu 02/08 · 23 de 23` na trivia,
`concluiu 02/08 · 548 de 548` na produção.

Clicando **aquecer** nas sagas, sem recarregar a página:

| | o cartão |
|---|---|
| ao clicar | o botão vira `rodando…` e desabilita |
| +6s | `20 de 233 · 1408` |
| +30s | `180 de 233 · Patch Adams: O Amor é Contagioso` |

O número e o título vêm do `progress` que o job já publicava desde a R32 e que
ninguém lia.

**234 testes**, um novo: as sagas são encadeadas **depois** do `run_matching`
dentro do `start_scan`. Ele compara as posições das duas chamadas no arquivo,
porque inverter a ordem não quebra compilação, não quebra teste de tipo e não
aparece na tela — só faz o encadeamento não achar nada.

### O que NÃO foi exercido, e fica dito

**O encadeamento não foi rodado de ponta a ponta.** Fazer isso hoje significaria
varrer os 17.500 arquivos (2m20 na última medição) e, na sequência, identificar as
**4.410 obras sem match** deste servidor — que é acervo de verdade de três
pessoas, não fixture. A ordem das chamadas está travada por teste e o caminho é o
mesmo do botão, que foi exercido ao vivo; a corrente inteira será exercida na
próxima varredura de verdade.

## 57. R41 — o desafio onde se cai, e a loja abrindo

Duas coisas baratas do `IDEIAS-2.md`, e as duas aparecem todo dia.

### Os desafios no "para você"

> *"Colocar os desafios também na aba para você"*

Eles moram no perfil desde a R35, e o perfil **é onde se vai de propósito**; o
"para você" é onde se cai. Um desafio que só existe na tela que se visita de
propósito é um desafio que se esquece.

**O componente saiu do `Perfil.tsx` e virou arquivo.** Copiar a lista pra segunda
tela criaria dois lugares pra consertar o mesmo desafio, e eles divergiriam no
primeiro conserto — é a mesma razão que fez o `so_manual` do §53 virar função
usada por cinco rotas.

**A cadência não foi junto**, e isso é a regra e não uma economia: ela é ajuste, e
ajuste não se repete em duas telas. Continua só no perfil, que é onde se vai
mexer nas suas coisas.

**E ele ficou fora do estado frio.** A faixa "continue de onde parou" só existe
quando `conhecimento < 1` — menos de seis sinais. Pendurar os desafios ali dentro
os faria sumir exatamente quando o Odeon passa a te conhecer, que é quando a
pessoa usa mais o produto. Medido nesta conta: `conhecimento >= 1`, a faixa não
renderiza, e os desafios aparecem assim mesmo — logo abaixo da barra de tempo e
antes do que a curadoria sugere. **O que você se comprometeu a fazer vem antes do
que a máquina achou.**

### A loja abrindo, no lugar do spinner

> *"Adicionar um loading legal na Locadora"*

O que havia era a frase *"acendendo as luzes…"* e mais nada: as quarenta caixas
chegavam juntas e a página saltava quando chegavam.

As prateleiras agora nascem com **a madeira desenhada e vazias**, e as caixas caem
uma a uma na ordem da estante — 34ms entre caixas, 90ms entre estantes, com teto
de oito estantes pra cascata não ficar mais longa que a paciência de quem só quer
pegar um filme. É a mesma escolha da grade de capítulos do §47 (moldura vazia em
vez da palavra "carregando"), com o vocabulário da loja.

**A queda é `translate`, e não `transform`** — e essa é a única sutileza técnica
do trabalho. A caixa já tem uma pose 3D (`rotateX(3deg) rotateY(22deg)`) e um
`transition` de `transform` que o hover usa: animar `transform` apagaria a pose
durante a queda, e a caixa cairia plana e giraria de repente ao pousar. A
propriedade `translate` compõe com a `transform` em vez de substituí-la.

`animation-fill-mode: backwards` é o que segura cada caixa invisível durante o
próprio atraso — sem ele as quarenta aparecem no primeiro quadro e só então
começam a cair, que é o oposto do que se quer. E a animação roda na montagem e só
nela: devolver uma fita não faz a loja inteira cair de novo.

### O salto de 16px que a medição achou

A primeira versão reservava, na prateleira vazia, a altura de uma caixa de DVD
(184px) mais o respiro do hover (46px) = 230px. **Medido no navegador: das 13
estantes desta loja, 7 mediam 230px e 6 mediam 246px** — porque a fita é 16px mais
alta que o disco, e a estante que tem uma fita cresce.

Ou seja: metade das estantes ia saltar 16px na chegada das caixas, que é
exatamente o defeito que este trabalho existia pra remover. Reservar o outro
número inverteria o problema.

O conserto não foi escolher melhor: **a fileira passou a ter piso fixo**, o da
caixa mais alta da loja. Agora a reserva bate por construção em vez de por acerto,
e as 13 estantes medem 246px em qualquer instante. O ganho veio dobrado — com
todas as prateleiras da mesma altura, as tábuas se alinham entre estantes de
conteúdo diferente.

### Verificação

Em Firefox headless, com `getAnimations()` contando o que ainda está no ar:

| instante | estantes vazias | caixas | no ar | alturas distintas |
|---|---|---|---|---|
| ao entrar | **4** | 5 | 5 | `[246]` |
| +0,7s | 0 | 54 | **49** | `[246]` |
| +7s | 0 | 54 | **0** | `[246]` |

Uma altura só nos três instantes — nada se moveu. E a pose 3D sobreviveu à queda:
a matriz computada da primeira caixa é `matrix3d(0.927184, 0.0196054, …)` no meio
do voo, e não uma translação pura.

Os desafios, nas duas telas:

| | "para você" | perfil |
|---|---|---|
| classe | `desafios compacto` | `desafios` |
| cadência | **ausente** | `todo dia · 3 em 3 dias · toda semana`, com "toda semana" ligada |
| itens | 3 | 3 |

O typecheck passa e os **234 testes** continuam verdes — nenhum é de front, que é
a assimetria mais antiga desta base e não é desta rodada.

## 58. R42 — a sala de gente, e a marca que ainda não é avatar

> *"Lista de amigos"*

**Decidido: melhorar o que já existe em mural › gente** — sem aba nova, sem
painel lateral. O que muda é o que cada linha diz, não onde ela mora.

A sala listava **nome e um botão**, e era tudo. Entraram quatro coisas, e
**nenhuma é dado novo** — as quatro já existiam em algum lugar do produto e não
chegavam nesta tela:

| | de onde veio |
|---|---|
| a marca da pessoa | desenhada do nome, aqui |
| o que está vendo agora | a **presença**, que o mural já busca de 30 em 30s |
| falar com ela | a sala de conversas, ao lado |
| o perfil dela | a aba perfil, que já sabia abrir o de outra conta (§48) |

A presença não ganhou uma segunda consulta: a sala recebe a mesma lista que o
painel lateral do mural já usa e cruza por id. Duas requisições pra mesma
pergunta dariam à tela a chance de discordar de si mesma sobre quem está online
— que é o argumento do §49 pras duas listas da presença, aplicado de novo.

### A marca não é o avatar do §4.1, e é de propósito

O `IDEIAS-2.md` §4.2 pede "avatar", e o §4.1 **decide** o que ele será: vários
prontos pra escolher, parte deles atrás de conquista, sem upload. Isso é a fase
seguinte, e não há coluna nenhuma no banco hoje.

O que entrou aqui é a **marca de quem ainda não escolheu**: um disco desenhado
em SVG com a inicial, a cor tirada do nome por hash e uma figura geométrica
entre quatro. Mesma conta, mesma marca, em toda tela e em toda sessão.

Isso não é trabalho jogado fora quando o §4.1 chegar: **ninguém nasce com avatar
escolhido**, e a marca continua sendo o padrão dessa conta. É a régua do §12 —
*zero bytes* —, a mesma que recusou CDN de fonte, sintetizou a trilha do menu
(§47) e desenhou o ícone de controles da barra (§52).

Quatro figuras e não doze: a marca serve pra reconhecer alguém numa lista de três
a dez pessoas, e nisso a cor já faz quase todo o trabalho — a figura é o que
separa duas contas que caíram em cores parecidas. Neste servidor isso já
aconteceu: `rudney` e `r42teste` começam com a mesma letra.

E a figura fica atrás da inicial, bem apagada: ela é textura, não desenho. Uma
figura que compete com a letra deixa a lista mais difícil de ler, que é o oposto
do que um avatar faz numa lista.

### As duas portas, e o que elas não oferecem

"falar" e "perfil" são atalhos, e têm peso menor que a ação de amizade —
"desfazer" é uma decisão, e as três com o mesmo peso competiriam.

**"falar" não aparece na busca.** Falar é entre amigos (§49), e oferecer o botão
pra quem vai levar recusa é o produto mentindo pra si mesmo — a mesma regra que
o §53 aplicou ao botão de editar o grafo.

E "o que está vendo" **não é link**: o cartaz da obra alheia é uma segunda
decisão, e esta linha é sobre a pessoa.

### Dois estados que a tela poderia ter errado

**O atalho é de uma vez só.** A sala de conversas remonta a cada visita, e um
`useEffect` com o valor ainda guardado reabriria, dias depois, a conversa de quem
foi atalhado uma vez. Um atalho que se repete sozinho deixou de ser atalho —
então quem consome avisa, e o valor é limpo.

**Ir pro perfil pela barra é sempre ir pro seu.** Sem isso, quem espiou o perfil
de um amigo pela sala de gente encontraria o dele de novo ao clicar em "perfil"
no menu, e concluiria — com razão — que o menu está quebrado. E a `key` do
componente troca com a pessoa olhada, porque `Perfil` semeia o estado a partir da
prop: sem ela, o segundo clique num amigo diferente não mudaria nada.

### Verificação

Com uma conta descartável (`r42teste`) online e com um `playback_state` fresco,
em Firefox headless:

| linha | marca | o que está vendo | anel | botões |
|---|---|---|---|---|
| gabriel | ✓ | — | não | falar · perfil · desfazer |
| **r42teste** | ✓ | **"vendo Drive"** | **sim** | falar · perfil · desfazer |
| rudney | ✓ | — | não | falar · perfil · desfazer |

Clicar em "falar" na linha do `r42teste` abre a sala de conversas **já com ele
selecionado** (`.conversa-item.on` = `r42teste`); clicar em "perfil" abre o
perfil dele. As duas coisas não faziam nada antes porque não existiam.

A conta de teste, a amizade, a sessão e o `playback_state` dela foram apagados
depois — `DELETE FROM app_user` e o cascade limpa o resto. **Nenhuma opinião
inventada ficou atribuída a pessoa real.**

Typecheck limpo; os **234 testes** seguem verdes e nenhum deles é de front, que é
a assimetria mais antiga desta base.

## 59. R43 — o perfil como o da Steam, e o endereço que o projeto não tinha

Quatro coisas foram decididas no `IDEIAS-2.md` §4.1: **rosto e capa escolhidos
de um conjunto pronto, vitrine montável, perfil com URL, e uma moldura que sai
das conquistas.** Esta seção entrega as quatro, e duas saíram diferentes do que
a proposta dizia — por decisão de quem decide, nos dois casos.

### O SVG que não foi

A proposta era desenhar os avatares em SVG, pela régua de zero bytes do §12.
Quem decide vetou:

> *"o rosto de alguns atores, diretores, pessoal de música… e algumas capas que
> são capas usadas em próprios filmes"*

É melhor, e por um motivo que a proposta não tinha visto: **a arte já está no
disco.** Medido antes de escrever uma linha:

| | com imagem no cache local |
|---|---|
| atores | **5.606** |
| diretores | **381** |
| compositores | **249** |
| filmes | todo identificado tem backdrop |

O rosto não custa um byte novo a servir — é o mesmo `/artwork/…` que a ficha já
usa desde o M2. **Fica zero bytes e fica do acervo de quem olha**, que é mais do
que o desenho geométrico entregaria.

### O catálogo é código, e o vínculo é temático

Mesma decisão da lista de conquistas (§48) — *"quem escreve a lista é quem
programa"*. Dezoito linhas em `enfeites.rs`: doze rostos, seis capas, quatro
cores. Metade aberta, metade atrás de conquista, como o §4.1 pediu — e há teste
que falha se essa proporção mudar.

**O vínculo não é sorteado**, e é o que faz ele valer alguma coisa:

| rosto | abre com |
|---|---|
| Sigourney Weaver | 10 de ficção científica |
| Quentin Tarantino | 10 de crime |
| David Fincher | 10 de madrugada |
| Uma Thurman | maratona de 6 num dia |
| Steven Spielberg | sete décadas |
| Hans Zimmer | Cinéfilo — 100 obras |

As capas trancadas seguem a mesma regra pelo gênero do filme: *Akira* com dez de
animação, *Corra!* com dez de terror, *Duna* com cinquenta de ficção científica.

Cada entrada aponta pra uma pessoa **pelo nome** e pra um filme **pelo título**.
Quem não está neste acervo não aparece na lista, em vez de virar moldura vazia
(§18) — o que torna o catálogo portátil de graça: o mesmo código em outro acervo
oferece outros rostos, sem migração e sem erro. **Neste, os dezoito resolveram.**

### O trancado aparece, e isso não contradiz o §48

A regra do §48 é que *a tela nunca ofereça o que a validação vai recusar*, e ela
continua valendo: a opção trancada não é clicável, e o servidor recusa a chave
com 403 — **medido**: `{"error":"este rosto ainda não está disponível pra você"}`
ao tentar gravar Hans Zimmer sem a conquista.

O que a tela faz é **mostrar** o trancado, em cinza, com o nome da conquista que
o abre. Esconder seria o erro que a própria lista de conquistas não comete ao
exibir as 80 com descrição: *"uma conquista secreta é uma conquista que ninguém
persegue"*. Um rosto secreto é a mesma perda.

### A vitrine tinha coluna e não tinha tela

A `vitrine` existe no banco desde o §17, com `CHECK (cardinality <= 6)`, e a
tela de escolher **nunca existiu** — a vitrine de todo mundo estava vazia porque
não havia como enchê-la. Agora há: busca, seis vagas, remover, e setas pra
ordenar.

Setas e não arrastar, ao contrário da tela de coleções: lá a lista tem dezenas
de itens e arrastar é o gesto certo; aqui são seis, e um alvo de arrastar de
84px numa lista de seis não é ganho, é chance de errar.

### O endereço, e por que entrou um router

O Odeon não tinha URL nenhuma: **as telas eram estado de aba**. O §4.1 pediu
*"um link que dá pra mandar"*, e a primeira proposta foi `pushState` em vinte
linhas, sem dependência — pela mesma régua de sempre.

Quem decide respondeu outra coisa:

> *"eu quero futuramente tudo linkável, então já coloca"*

E aí a conta vira outra. Um perfil endereçável se resolve à mão; **todas** as
telas endereçáveis — com filtro, histórico e voltar — é escrever um router pior.
`react-router-dom` é a quarta dependência deste front, e entra sabendo disso.

O que ele **não** resolve, e é o que a discussão esclareceu: o caminho bonito
precisa do servidor devolvendo `index.html` pra um caminho que não é arquivo.
Isso é do nginx, com router ou sem ele. Daí o `web/nginx.conf`, e ele foi
exercido de verdade — a imagem `runtime` foi construída e sondada:

| caminho | resposta |
|---|---|
| `/` | 200 |
| `/p/sam` | **200**, com o `index.html` |
| `/biblioteca` | **200** |

Sem essa configuração, um link compartilhado daria 404 e a página nem carregaria
— o router mora no navegador, e o navegador nunca teria recebido o navegador.

**A troca no `App.tsx` foi pequena de propósito**: o corpo continua desenhando
por `tab`, e o que mudou é de onde `tab` vem — do `useLocation`, e não de um
`useState`. Reescrever as onze telas como `<Routes>` aninhadas seria uma reforma
num arquivo de mil linhas pra chegar no mesmo lugar. As rotas são em português,
como o resto do projeto: `/biblioteca`, `/colecoes`, `/ao-vivo`, `/p/<nome>`.

**E o link é por nome, não por id.** A rota aceita os dois — o placar só tinha o
id —, mas o que se copia é `/p/sam`: um UUID num endereço que alguém digita ou
lê em voz alta é endereço de banco, não de gente.

**"Público" quer dizer dentro de casa**, e foi decidido assim: quem recebe o link
faz login como sempre. Abrir uma rota sem sessão seria o primeiro furo
deliberado no `require_auth`, e o §49 firmou que quem vê o que você faz é quem
você aceitou.

### Verificação

Em Firefox headless, **entrando direto pelo link**, sem passar pela home:

| | |
|---|---|
| `/p/sam` | abre o perfil, capa desenhada, rosto **Robin Williams**, cor `#e0b062` |
| `/locadora` | abre a locadora com a aba certa acesa |
| botão voltar | devolve `/p/sam` com o perfil na tela |
| botão de link | copia `/p/sam` |

As três galerias, com o estado de cada opção lido do DOM:

| galeria | abertas | trancadas |
|---|---|---|
| rosto | 6 | **6**, cada uma dizendo a conquista que a abre |
| capa | 3 | **3** |
| cor | 1 | **3** |

E a recusa do servidor conferida por fora: gravar um rosto trancado devolve 403
com a frase, em vez de gravar e mostrar sucesso.

**238 testes** — quatro novos, todos no catálogo: metade nasce aberta, toda
conquista citada existe de verdade, nenhuma chave se repete, e a moldura guarda
nome e cor separados. O último nasceu de um defeito que o screenshot achou: a
galeria de cores mostrava `#4ea36b — abre com a conquista "Sócio"`, que é
endereço de memória pra quem está escolhendo uma cor.

### O que ficou de fora, e fica dito

**A tela do perfil é a única endereçável de verdade.** As onze abas têm caminho,
mas o estado interno delas não: o filtro da biblioteca, a obra aberta na ficha e
a sala do mural continuam invisíveis na URL. Isso é o que falta pra "tudo
linkável" valer a palavra, e agora é barato — o router já está aqui.

**Os enfeites do `sam` foram desfeitos depois do teste.** Rosto, capa, cor e uma
bio de exemplo foram gravados pra fotografar a tela e removidos em seguida: a
escolha é dele, e uma bio inventada num perfil de pessoa real é exatamente o que
o §18 chama de mentir com cara de metadado.

## 60. R44 — a notificação do agendamento, e o barramento que estava morto

> *"Adicionar uma notificação para receber os agendamentos"*

O `IDEIAS-2.md` §3.6 decidiu que faltava **a notificação do sistema, fora da
aba**, e propôs `Notification` do navegador com a permissão pedida na hora de
agendar.

**Isso já estava no código** — como no §3.4, escrito no commit 515b55c, um dia
antes do documento. A permissão era pedida no clique; o `new Notification` era
disparado no evento `programme_starting`. Tudo escrito, e nada funcionando.

### O defeito: `/api/events` recusava toda conexão de navegador

`EventSource` **não manda header**. É a mesma limitação de `<img>` e `<video>`,
e é exatamente pra isso que existe a lista `accepts_query_token` do §43 — as
rotas que aceitam `?token=` porque quem as busca é o próprio HTML.

`/api/events` **não estava nessa lista**. Medido, e o contraste é a prova:

| pedido | resposta |
|---|---|
| `GET /api/events` com header de sessão | **200** |
| `GET /api/events?token=<mídia>`, que é o que o navegador faz | **401** |

E a API do `EventSource` reage a 401 **reconectando pra sempre, calada**: sem
erro na tela, sem log no cliente, sem nada. No navegador, `readyState = 2` a
cada tentativa.

O que estava morto por causa disso, tudo do M3 em diante:

- o aviso de programa agendado — o pedido desta fase;
- as atualizações ao vivo do mural;
- o pedido de fita de volta na locadora, que o §49 chamou de *"o que separa uma
  rede social de um relatório"*;
- a sincronia do player entre aparelhos.

### E, atrás dele, mais dois

Consertar a lista não bastou, e os dois seguintes só apareceram porque a
verificação continuou até o aviso **chegar de verdade na tela**.

**A URL congela na criação.** O `renovar()` do boot não é esperado por ninguém —
*"a arte carrega quando ele chegar"*, e pra uma `<img>` isso é verdade porque
ela recarrega. Um `EventSource` monta a URL uma vez: nascido antes do token, ele
nasce errado e morre assim. Quem abria o Odeon direto numa tela com barramento
nunca recebia evento; quem passeava por outras abas antes recebia, porque o
token tinha chegado no meio do caminho. **Daí o defeito parecer intermitente.**

**Emitir um token de mídia aposenta o anterior** (§43) — e isso mata a conexão
aberta com o velho. Duas abas, um `StrictMode` que monta duas vezes, ou oito
horas de sessão: qualquer um dos três derruba o barramento. Medido no
contador instrumentado: `erros: 2, msgs: 0` no app enquanto um `EventSource`
cru, aberto na mesma página com o token da vez, recebia tudo.

O conserto dos dois é um só, e agora mora num lugar só: `api.ouvirEventos()`
espera o token existir, reconecta com token novo quando a conexão cai, e desiste
depois de cinco tentativas em intervalos crescentes. **Quatro telas escreviam as
mesmas seis linhas** — App, mural, locadora e player —, e três delas com o mesmo
defeito.

### O aviso que chegava quando ninguém ouvia

Outro achado da mesma investigação, e este é de desenho: o vigia marca
`notified_at` e publica **uma vez**. Se naquele instante não havia aba aberta, o
evento morre no ar e o lembrete nunca mais dispara.

Não precisa de service worker pra melhorar: na abertura, o Odeon pergunta o que
está agendado — rota que já existia — e recupera o que **começou há menos de
quinze minutos e ainda está no ar**. Quem abre cinco minutos depois vê que
começou; quem abre no dia seguinte não vê nada, que é o certo (§18).

E o aviso recuperado diz *"já começou"*, não *"começando"*: são fatos
diferentes, e trocar um pelo outro manda a pessoa correr pra pegar o início de
um filme que já vai na metade.

**O aviso só é marcado como lido quando sai da tela** — no clique ou no fim dos
vinte segundos. Marcar ao enfileirar parece igual e não é: se a tela fechar
antes de ele aparecer, fica lido sem ter sido visto. Foi exatamente assim que o
defeito apareceu, num remount do modo estrito que engoliu o primeiro aviso — a
mesma falha do `notified_at`, um andar acima.

### O que mais faltava, e não era notificação

**A rota dos agendamentos não tinha cliente.** `GET /api/live/reminders` existe
desde a R17: dava pra agendar um programa e **não dava pra ver o que estava
agendado**. É o §27 pela terceira vez nesta rodada. Agora há uma faixa "Você
agendou" entre a sintonia e a linha do tempo, que some quando não há nada (§24).

**E o que o navegador respondeu passou a ser dito.** Agendar com a permissão
negada deixava o botão verde e a notificação nunca chegava — o produto mostrando
sucesso pra um recurso que ele sabia que não ia entregar. Agora a tela diz qual
dos três casos é: sem suporte, bloqueado no site, ou permissão não concedida. E
não é erro: o agendamento funcionou, e o aviso dentro do Odeon continua vindo. O
que mudou é o alcance.

### Verificação

Em Firefox headless, com uma espiã no lugar do `Notification` do navegador — ela
não testa o navegador, testa se o Odeon **chama**, e com o quê:

| | |
|---|---|
| a faixa "Você agendou" | três programas, com canal e `hoje 16:04` |
| aviso recuperado na abertura | `● JÁ COMEÇOU · Uma Família da Pesada · Sessão Seriado` |
| **aviso ao vivo** | `● COMEÇANDO · Zedin · Videoteca` |
| **notificação do sistema** | `{"titulo":"Começando agora no Odeon","body":"Zedin · Videoteca","tag":"odeon-programa-9373"}` |

E a reconexão, provada no cenário que matava tudo: com a página aberta, um token
de mídia novo é emitido **do lado de fora** (aposentando o da conexão), e em
seguida um evento é publicado. O app recarrega a biblioteca — de 1 pra 2
requisições — porque recebeu o evento pela conexão refeita.

**239 testes**, um novo: `/api/events` aceita token na query. Tirá-lo da lista
mata o aviso, o mural ao vivo, o pedido de fita e a sincronia do player de uma
vez só, **e sem nenhum erro em lugar nenhum** — que foi como ficou quebrado sem
ninguém notar.

Ficou também uma linha de log por conexão ao barramento. É barato, e é o que
teria denunciado isto no primeiro dia.

### O que isto NÃO fecha

**Sem service worker, o aviso continua exigindo o Odeon aberto** — em segundo
plano basta, com a aba fechada não. Era a decisão do §3.6 e ela continua de pé;
o que mudou é que agora o caso que ela cobre funciona de verdade.

**Os canais da casa não podem ser agendados.** `programme_reminder` aponta pra
`programme`, que é a grade do IPTV; os canais do Odeon são calculados (§25) e não
têm linha. Depois da R39 o bloco deles abre o cartaz da obra, que não sabe de
horário. É o mesmo cidadão de segunda classe que a R39 encontrou, num outro
lugar — e fechar isso é uma decisão de desenho, não um conserto.

## 61. R45 — o rebobinar, e a dívida mais antiga em aberto

> *"Animação vhs rebobinar"*

O §46 já tinha anotado o que faltava, com todas as letras: *"hoje é um ponteiro
regressivo e um carretel andando pra trás — falta o objeto girando, o ruído e o
tranco no fim"*. Esta seção paga as três.

### O objeto

O que havia era **um anel de CSS girando ao contrário**. O que há agora é a
janela de um VHS com os dois carretéis — os mesmos que a caixa já desenha na
estante, na mesma linguagem, porque é a mesma fita.

Três coisas que o anel não tinha, e cada uma diz algo que o anel não dizia:

| | e por que importa |
|---|---|
| **os dois giram, em sentidos opostos** | é o que os carretéis de uma fita fazem: um entrega, o outro recolhe |
| **a velocidade cai com o que falta** | a fita sai rápido e vai perdendo força — o ponteiro virando movimento |
| **o rolo da esquerda engorda** | a fita voltando pro lugar de onde saiu, que é o que "rebobinar" quer dizer |

**O giro não vem de `@keyframes`.** Uma animação de velocidade constante não
sabe desacelerar junto com um número que veio do banco — então o ângulo é escrito
como propriedade CSS a cada quadro, acumulado em vez de calculado do tempo (o
disco daria um salto quando a velocidade mudasse). É a mesma decisão da agulha
do "ao vivo" (§25): o React é acordado só quando o **segundo** muda, e não
sessenta vezes por segundo pra mover dois discos.

### O ruído

Sintetizado, pela régua de zero bytes do §12 — a mesma que recusou CDN de fonte
e que fez o menu de DVD sequenciar a trilha em vez de servir um `.ogg` (§47). Um
sample de rebobinar custaria uns 100 KB e uma licença pra alguém conferir.

E é historicamente correto pelo mesmo motivo que a trilha do menu era: o som de
um VHS **não é uma gravação**, é um motor e um atrito. Três camadas, e cada uma
é uma peça do aparelho:

| camada | o que é no objeto |
|---|---|
| ruído branco por um passa-faixa | a fita raspando na cabeça e nas guias |
| dente de serra grave | o motor puxando o carretel |
| seno agudo, baixinho | o assobio da engrenagem em rotação alta |

As três respondem à **mesma velocidade que gira os carretéis** — som e imagem
contam a mesma coisa, e é isso que faz o conjunto convencer. O `Q` do filtro é
baixo de propósito: passa-faixa estreito vira apito, e fita não apita, chia.

### O tranco

*"A parada seca com um pulo de um quadro."* Ele é metade imagem e metade som: a
fita inteira recua 3px e volta, uma vez, em 160ms, enquanto o motor cai de tom
e entra um baque grave curto — o mecanismo batendo no fim de curso.

Sem o baque, o silêncio lê como o áudio tendo acabado, e não como a fita tendo
chegado. Sem o pulo, o movimento simplesmente para — e **parar não é chegar**.

### Duas coisas que a medição corrigiu

**O objeto nasceu pequeno demais.** Na primeira medida a fita tinha 190px e os
dentes ficavam com 12px: um disco girando pequeno demais é um disco parado. O
screenshot mostrou dois pontos escuros. Foi pra 268px, com os dentes em contraste
alto — são eles que provam a rotação.

**E a variável mentia.** Ela se chamava `--cheio` e valia `1 - t`, ou seja: o
nome dizia "quanto já voltou" e o valor era "quanto ainda falta". Virou
`--restante`, que é o que ela é — o mesmo número do ponteiro, normalizado.

### `prefers-reduced-motion`, e por que ele precisou de linha própria

A regra global do CSS mata `animation` e `transition` — e **não alcança um
ângulo escrito por JS**. Quem pediu menos movimento continuaria vendo dois discos
girarem por até dez segundos. Agora eles ficam parados, e a espera — que é o
conteúdo do gesto, e não o enfeite — continua igual. *"Alma não pode custar
enjoo"* (§52).

### Verificação

Em Firefox headless, lendo as propriedades computadas quadro a quadro durante um
rebobinar de verdade:

| instante | `--giro-a` | `--giro-b` | `--restante` |
|---|---|---|---|
| +0,6s | **−113,8°** | **+72,8°** | 0,82 |
| +1,1s | −147,3° | +94,3° | 0,67 |
| +1,7s | −188,6° | +120,7° | 0,34 |
| +2,2s | −213,6° | +136,7° | 0,02 |
| +2,8s | −226,1° | +144,7° | **0** · `trancou` |

Os sinais opostos são os sentidos opostos. O passo entre as amostras encolhe —
de 41° para 12° — que é a velocidade caindo com o que falta. E o `--restante`
indo a zero é o rolo trocando de lado.

**239 testes** e typecheck limpo; nenhum teste novo, porque nada disto é regra —
é desenho, e quem verifica desenho é o olho e o relógio.

O gesto foi exercido com uma **conta descartável** (`r45teste`), que pegou uma
fita emprestada, rebobinou e devolveu. A conta foi apagada em seguida e o
empréstimo caiu com ela pelo `ON DELETE CASCADE` — **as duas fitas que estão no
meio neste servidor continuam onde as pessoas as deixaram**, que é o dado que
esta fase não podia tocar.

## 62. R46 — assistir junto, e as três perguntas respondidas antes do código

> *"Watch Party (Interação fácil entre amigos)"*

O `IDEIAS-2.md` §4.6 decidiu o que é — **assistir junto de verdade sincronizado,
mais conversa ao lado** — e respondeu as três perguntas do desenho antes de
existir uma linha. Este módulo é a transcrição delas.

### 1. Quem manda é o host

Existe um dono da sessão, e é dele o controle. Isso resolve sozinho a briga de
dois cliques simultâneos: **não há eleição, não há empate**, e o estado tem uma
fonte só.

Na tela do membro os controles **não ficam desabilitados — eles não existem**,
com uma frase no lugar: *"quem manda é sam"*. Um controle apagado convida a
tentar, e tentar aqui é levar um "não" que a tela já sabia (§53). E o servidor
recusa de qualquer jeito: `{"error":"quem manda na sala é o host"}`.

### 2. Quando um trava, todo mundo para

*"Sempre sincronizado."* Não há modo tolerante: se a sessão é assistir junto,
assistir separado por trinta segundos é a sessão tendo falhado em silêncio.

A regra inteira cabe numa linha do servidor, e há teste que quebra se ela virar
duas:

```rust
rodando: tocando && esperando.is_empty()
```

`tocando` é a **intenção** do host; `rodando` é o que toca. Cada tela avisa
quando está carregando e quando voltou, e o servidor soma. **Ausente não
segura** — quem sumiu há mais de dois minutos já não está na sessão, e esperar
por uma aba fechada travaria a sala pra sempre.

**A consequência foi dita antes de ser sentida** (§4.6): a conexão mais lenta
manda no ritmo de todo mundo. Por isso a sala mostra **o nome de quem está
segurando** — *"esperando r46teste carregar…"* —, e por isso a rota de expulsar
existe desde o primeiro dia. O conserto não é afrouxar a sincronia; é uma
decisão social.

### 3. Os dois modos de stream, como opção da sessão

`por_pessoa` é o padrão — é o que já funcionava sem código novo. O
`compartilhado` guarda o `transcode_id` do host, e os membros leem a playlist
dele: **a única coisa que esse modo custou** foi uma permissão estreita
(`pode_ler_transcode`), válida só enquanto a sala está aberta, só pra quem está
dentro, e só pra sessão que aquela sala declarou.

### O transporte é o barramento, e o estado é a tabela

*"Ele é o transporte, e não se inventa um segundo canal."* O evento diz apenas
**qual sala mexeu**; o estado mora em `sessao_junta`. Quem entra atrasado lê e
chega no ponto certo — o oposto do defeito que a R44 encontrou no aviso de
programa, onde o evento publicado no vazio sumia pra sempre.

E não há tabela de convite: **a amizade já é o aceite** (§44). Uma sala aberta
aparece no mural de quem foi aceito, e de mais ninguém.

### Três defeitos que esta fase encontrou — dois deles recém-criados

**O `midia.clear()` da reconexão do barramento** (R44) aposentava o token que
está dentro do `<video src=…?token=>`. O segundo participante recebia *"o
navegador recusou o arquivo mesmo com plano de Direct Play"* — era o barramento
derrubando o filme dele. A reconexão passou a usar o token que estiver valendo.

**O `pronto` virou laço.** A rota devolve a sala; a sala trocava de identidade;
o efeito rodava de novo e avisava outra vez. Uma fila infinita de
`/api/junto/…/pronto`.

**E quatro `EventSource` abertos** — App, aviso de programa, salas abertas e
mural — comiam o orçamento de **seis conexões por host** do navegador. O sintoma
foi o pior possível: no segundo participante, o `fetch` do plano de reprodução
**ficava trinta segundos pendurado**, o vídeo nunca carregava, e a sala inteira
esperava por ele. Agora é **uma conexão pro aplicativo inteiro** — que é o que o
`events.rs` documentava desde o M3: *"o navegador já mantém UM `EventSource`
aberto"*.

### E o impasse que a própria regra criava

A régua de "pronto" era `readyState >= 3`. Numa sala isso **trava sozinho**: a
sala nasce parada, o vídeo de todo mundo começa pausado, e um vídeo pausado pode
nunca passar de `HAVE_CURRENT_DATA` porque o navegador não vê motivo pra encher
o buffer. Ninguém fica pronto, nada toca — e nada tocar é justamente o que
impede de encher o buffer. O impasse se alimenta.

A régua passou a ser `>= 2` — *tenho o quadro deste ponto* —, que é a promessa
honesta pra sincronia, mais `preload="auto"` no elemento. Quem travar de verdade
no meio avisa pelo `waiting`, que é o evento que existe pra isso.

### Verificação

Com duas contas de verdade e **um navegador de cada vez** — dois Firefox
simultâneos lotam o `/tmp` da máquina, e não são necessários: cada metade da
sincronia se prova sozinha.

**Fase 1 — o host publica.** O navegador é o host:

| | |
|---|---|
| depois do play | `tocando=true` · `rodando=true` · ninguém esperando |
| depois da pausa | `tocando=false` · `rodando=false` · **posição 5,1s** |

A posição em 5,1s é a prova de que o filme rodou de verdade entre um e outro.

**Fase 2 — o membro obedece.** O navegador é o membro, e o host age só por HTTP:

| | |
|---|---|
| o convite no mural | *"sam está assistindo junto · Drive · 1 pessoa"* |
| ao entrar | `readyState 4`, sem controles, *"QUEM MANDA É SAM"* |
| host manda **tocar** | o membro tocou em **~2s** (`pausado: false`, t=13,2) |
| host manda **parar** | o membro parou em **~2s** (t=12) |
| membro tentando mandar | `{"error":"quem manda na sala é o host"}` |

A conversa foi provada antes, com as duas telas abertas: *"r46teste passa o
pão"* escrito pelo membro apareceu na tela do host.

**241 testes**, dois novos e os dois sobre a regra que não pode escorregar: que
`rodando` exige todo mundo pronto, e que ausente não segura a sala.

### Uma coisa que o teste ensinou sobre o produto

Um navegador morrendo **publica a pausa**: a aba do host, ao fechar, pausa o
vídeo, e o `onPause` do player manda `tocando=false`. No teste isso era ruído —
e no produto é o comportamento certo: quem fechou a aba parou de assistir, e a
sala saber disso é melhor do que a sala esperar por um fantasma.

### O que NÃO está fechado

**O modo compartilhado não foi exercido.** A permissão existe, a coluna existe e
o host publica o `transcode_id` — mas neste acervo o arquivo testado é Direct
Play, e sem transcode não há sessão pra compartilhar. Falta uma prova com um
arquivo que exija transcode.

**Não há reencontro depois de fechar a aba.** Quem fecha continua membro, e ao
voltar cai na sala de novo — mas o filme dele reabre do ponto da sala, não de
onde ele parou. É o certo pra sincronia e é uma escolha, não um esquecimento.
