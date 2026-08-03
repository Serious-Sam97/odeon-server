# Odeon — segunda rodada

## Sobre este arquivo

O `IDEIAS.md` é o registro fechado da primeira rodada: onze anotações, oito
fases, tudo feito e documentado no `DESIGN.md` §44 a §51. Ele não se mexe — o
`DESIGN.md` aponta pra ele em oito seções.

Este é o começo da segunda. Mesma regra de autoria, e ela vale para o documento
inteiro:

- **Decidido** — palavra de quem decide, dita explicitamente. Não se mexe sem
  perguntar.
- **Proposto** — sugestão de quem escreve, esperando confirmação. Pode ser
  vetada sem discussão.
- **Medido** — número tirado do acervo real, hoje.

Onde não houver marca, é fato de código.

E a regra de trabalho da primeira rodada continua valendo, porque foi ela que
consertou aquela: **quando a ideia parecer "errada" pela régua de engenharia,
perguntar ou fazer o que foi pedido — nunca entregar a versão sóbria por conta
própria.**

---

## 0. O estado, medido em 03/08/2026

| | |
|---|---|
| usuários | **3** — `sam` (admin), `rudney` e `gabriel` (ambos `user`) |
| obras · filmes · séries | 17.498 · 635 · 115 |
| coleções | **709**, todas `provider` — 133 sagas, 461 temporadas, 115 séries |
| coleções `manual` ("suas ordens") | **0** |
| histórico | 129 eventos, 2 obras terminadas, 1 pessoa |
| conquistas · desafios | 80 definidas · 3 por janela |
| testes · CI | **229** · nenhum |

Dois números que decidem coisas neste documento:

> **Das 709 coleções, 131 sagas têm pôster remoto e 113 séries têm pôster
> local.** É a diferença entre o pipeline antigo, que baixa a arte, e o job de
> sagas da R32, que não baixa.

> **Ninguém criou uma "sua ordem" ainda.** A feature existe desde o §17 e tem
> zero linhas — o que torna barato mudar as regras dela.

---

## 1. As anotações, como foram escritas

Texto original, sem edição. Tudo neste documento responde a ele.

```
* Perfil pessoal customizado como Steam
* Em Coleções dentro de uma franquia a lista deveria estar ordenado por ano
* Como dou refresh nas coleções para pegar filmes novos?
* Adicionar um loading legal na Locadora
* Colocar os desafios também na aba para voce
* Lista de amigos
* Verificar se usuario normal tem acesso a qualquer configuração do servidor,
  PRINCIPALMENTE usuario normal nao pode apagar nem modificar nada
* Em guia o cartaz da semana ta quebrado
* Na real diversas capas no guia estao quebradas
* AO VIVO: Na linha do tempo eu nao consigo clicar em cima dos canais odeon
* Adicionar uma notificação para receber os agendamentos
* Alguns filmes estao com o player estranho, começa com um tempo de filme mega
  pequeno e vai aumentando ao longo que vai carregando sei la
* Watch Party (Interação facil entre amigos)
* Animacao vhs rebobinar
```

E, junto: *"as coisas experimentais não precisam ficar pra sempre no
experimental; se terminamos já tá de boa pra tirar e colocar no seu lugar"* —
que virou a R36 (§52) antes deste documento existir.

---

## 2. O que separa esta rodada da primeira

A lista acima **não é uma lista de features**. Sete dos catorze itens são
defeitos, e três deles nasceram na rodada passada, na semana em que ela foi
escrita. Isso muda a ordem de ataque: **conserto antes de construção.**

Um pedaço já foi feito enquanto este documento era planejado:

| | |
|---|---|
| ✅ **permissões** | R37, `DESIGN.md` §53 — morador comum não edita mais o acervo |
| ✅ o menu superior | R36, §52 — "experimentação" acabou |

---

## 3. Os defeitos, com causa medida

Nenhum destes é palpite: todos foram reproduzidos ou lidos no banco.

### 3.1 As capas quebradas do guia — **um defeito, não dois**

> *"Em guia o cartaz da semana tá quebrado"* · *"Na real diversas capas no guia
> estão quebradas"*

**Medido:** as 131 sagas com pôster guardam **caminho remoto do TMDB**
(`/mv0MySTq….jpg`); as 113 séries guardam **arquivo local** (um UUID no cache).

**A causa é minha, da R32.** O `metadata/saga.rs` gravou o `poster_path` cru
sem chamar `artwork::fetch` — que é exatamente o que o pipeline de série faz
desde o M1. Aí `/artwork/…` responde 404 e a moldura fica vazia.

Os dois itens da lista são o mesmo bug: o "cartaz da semana" quebra quando o
evento é uma saga, e as "diversas capas" são as sagas na capa do guia.

**Proposto:** baixar a arte no próprio job de sagas, e uma varredura única pras
131 que já existem. É o mesmo `artwork::fetch` de sempre — não há decisão nova
aqui, só uma chamada que faltou.

### 3.2 A ordem dentro de uma franquia

> *"Em Coleções dentro de uma franquia a lista deveria estar ordenado por ano"*

A consulta é `ORDER BY ci.position NULLS LAST, w.title`. As sagas do TMDB vêm
**sem `position`**, então cai no alfabético — e é por isso que *Câmara Secreta*
aparece antes de *Pedra Filosofal*.

**Proposto:** dentro de uma coleção `provider`, ordenar por **ano** e cair no
título só no empate. A `position` continua mandando onde ela existe, porque é
ela que carrega a ordem Machete e as ordens manuais — que são opinião, e opinião
tem precedência sobre cronologia.

### 3.3 Os canais Odeon não abrem na linha do tempo

> *"AO VIVO: Na linha do tempo eu não consigo clicar em cima dos canais odeon"*

**Reproduzido.** O clique chega — `elementFromPoint` devolve o bloco,
`pointer-events: auto` — e nada acontece. A linha é esta:

```js
onAbrir={(b) => {
  const p = guia?.programas.find((x) => String(x.id) === b.id);
  if (p) setDetalhe(p);          // Odeon nunca acha, e some em silêncio
}}
```

O modal procura o bloco na **grade do IPTV**. Os canais da casa são programados
pela emissora (§25) *sem tabela* — os blocos deles não existem nessa lista.
`p` vem `undefined` e o `if` engole.

É o §8b inteiro: **errar em silêncio é o defeito**. Um clique que não faz nada
não é um recurso ausente, é um recurso quebrado.

**Proposto:** o bloco do canal da casa abre o **cartaz da obra** — ele aponta pra
uma obra do acervo, que é mais do que um programa de IPTV tem.

### 3.4 O player começa curto e vai crescendo

> *"Alguns filmes estão com o player estranho, começa com um tempo de filme mega
> pequeno e vai aumentando ao longo que vai carregando"*

A sessão HLS é criada com:

```
-hls_playlist_type event
-hls_list_size 0
```

O comentário no código explica a escolha, e ela é boa: `event` deixa a playlist
só crescer, então o player pode voltar pra qualquer ponto já produzido sem
sessão nova. **O preço não foi contabilizado:** sem `#EXT-X-ENDLIST`, o
`video.duration` é a soma dos segmentos prontos — ele nasce em vinte segundos e
cresce enquanto o ffmpeg trabalha.

E o dado certo já está na tela: `Player.tsx` conhece `work.duration_seconds`.

**Proposto:** a barra e o relógio passam a usar a duração da obra; a duração do
stream fica só pra saber **até onde dá pra pular agora**. É o §14 outra vez — um
número servido pelo servidor vale mais que um número que o navegador deduziu.

### 3.5 O job de sagas não tem botão

> *"Como dou refresh nas coleções para pegar filmes novos?"*

**A resposta honesta é: hoje, por `curl`.** A rota
`POST /api/maintenance/aquecer-sagas` existe desde a R32 — e eu nunca pus botão
nenhum. Ela é retomável por construção (o alvo é "filme sem franquia"), então
rodar de novo pega os filmes novos. Só não há como rodar.

É literalmente o defeito que o §27 corrigiu uma vez: *"sete rotas existiam sem
nenhum cliente, e quatro delas só eram alcançáveis por `curl`"*.

**Proposto:** botão na aba `admin`, ao lado dos outros aquecimentos, com o
progresso que o job já publica. E **proposto**, separado: a varredura chamar as
sagas no fim, pra "achei filmes novos" e "as sagas deles apareceram" serem um
gesto só.

### 3.6 A notificação do agendamento

> *"Adicionar uma notificação para receber os agendamentos"*

**Decidido:** o que falta é a **notificação do sistema, fora da aba**. O aviso
que existe hoje (`AvisoDePrograma`) só aparece com o Odeon aberto.

**Proposto:** `Notification` do navegador, com a permissão pedida **na hora de
agendar** e não na entrada — pedir permissão antes de a pessoa querer algo é
como se perde a permissão. Sem service worker por ora: com a aba aberta em
segundo plano já resolve o caso real, e um service worker é uma peça a mais pra
manter.

---

## 4. As ideias

### 4.1 Perfil pessoal, como o da Steam

> *"Perfil pessoal customizado como Steam"*

**Decidido**, e são quatro coisas:

| | o que é | onde está hoje |
|---|---|---|
| **avatar e capa** | escolhidos de um conjunto pronto | não existe — o perfil abre com o nome em texto |
| **vitrines montáveis** | escolher os filmes que aparecem | a coluna existe (até 6), **a tela de escolher não** |
| **perfil público com URL** | um link que dá pra mandar | só se alcança clicando no placar de amigos |
| **moldura/tema desbloqueável** | a cor do perfil sai das conquistas | não existe |

O quarto é o que amarra com a fase 5: são **80 conquistas** já definidas, e hoje
elas só rendem título e tag. **Proposto:** a moldura sai de uma conquista, como
o título — nada aparece no perfil que não tenha sido conquistado, com a bio como
a exceção já declarada (§48).

**Decidido sobre avatar e capa: são vários, prontos, e você escolhe.** Não há
upload. Alguns nascem abertos; os outros vêm das conquistas, como o título e a
tag já vêm hoje.

Isso apaga um problema inteiro antes de ele existir. Upload seria o **primeiro
do projeto**, e traria limite de tamanho, tipo aceito, validação de conteúdo e
um lugar no disco pra guardar arquivo que usuário mandou — quatro decisões novas
pra uma foto de perfil. Nada disso precisa acontecer.

E casa com o que o perfil já é: **tudo que aparece nele foi conquistado**, com a
bio como a exceção declarada (§48). Um avatar que se desbloqueia é a mesma
frase; um avatar que se sobe não é.

**Proposto:** os avatares e as capas são **desenhados em SVG/CSS**, e não
imagens. É a régua do §12 — *"zero bytes"*, que já recusou CDN de fonte e já
rendeu a trilha sintetizada do menu (§47) e o ícone de controles da barra (§52).
Uma dúzia de marcas geométricas na paleta da casa custa nada pra servir, escala
em qualquer tela e não tem licença pra ninguém checar.

**Proposto:** o conjunto começa pequeno e cresce — uma dúzia de avatares e meia
dúzia de capas, com metade atrás de conquista. Quem escreve a lista é quem
programa, como foi decidido pras conquistas (§3.3 da rodada 1).

### 4.2 Lista de amigos

> *"Lista de amigos"*

**Decidido:** melhorar o que já existe em **mural › gente** — sem aba nova, sem
painel lateral.

**Proposto**, o que falta lá: avatar, o que a pessoa está vendo agora (a
presença já sabe), atalho pra conversa e pro perfil dela. Hoje a sala lista
nomes e botões de adicionar/desfazer, e é tudo.

### 4.3 Desafios no "para você"

> *"Colocar os desafios também na aba para você"*

**Decidido.** Eles moram no perfil desde a R35, e o perfil é onde se vai de
propósito — o "para você" é onde se cai.

**Proposto:** a mesma lista, compacta, abaixo do "continue de onde parou". E a
cadência continua só no perfil: é ajuste, e ajuste não se repete em duas telas.

### 4.4 Um loading de verdade na locadora

> *"Adicionar um loading legal na Locadora"*

Hoje é a frase *"acendendo as luzes…"* e nada mais — as estantes aparecem de
uma vez quando as 40 caixas chegam.

**Proposto:** as prateleiras nascem vazias com a madeira já desenhada e as
caixas caem uma a uma, na ordem da estante. A loja abrindo, não um spinner. É a
mesma escolha da grade de capítulos do §47, que mostra molduras vazias em vez de
"carregando" pra nada saltar quando chega.

### 4.5 A animação do rebobinar

> *"Animação vhs rebobinar"*

Já anotada como dívida no §46: hoje é um ponteiro regressivo e um carretel
andando pra trás. **Falta o objeto girando, o ruído e o tranco no fim.**

**Proposto:** os dois carretéis girando em sentidos opostos com velocidade
proporcional ao que falta, o ruído no mesmo sintetizador Web Audio do menu
(§47 — zero bytes), e a parada seca com um pulo de um quadro. Mesma família de
trabalho do menu de DVD.

### 4.6 Watch Party

> *"Watch Party (Interação fácil entre amigos)"*

**Decidido:** assistir junto **de verdade sincronizado**, mais **conversa ao
lado** durante a sessão.

- quem pausa, pausa pra todos; quem volta, volta pra todos;
- a conversa fica guardada — e a mensagem direta da fase 6 já existe;
- é a maior coisa desta lista, e é uma fase inteira sozinha.

**Decidido, e as três perguntas do desenho estão respondidas:**

**Quem manda é o host.** Existe um dono da sessão, e é dele o controle. Isso
resolve sozinho a briga de dois cliques simultâneos — não há eleição, não há
empate, e o estado da sessão tem uma fonte só.

**Quando um trava, todo mundo para.** *"Sempre sincronizado."* Não há modo
tolerante em que os rápidos seguem e o lento se perde: se a sessão é assistir
junto, assistir separado por trinta segundos é a sessão tendo falhado em
silêncio. É a mesma régua do §8b.

> **Proposto**, e é a consequência que vale dizer: isso significa que **a
> conexão mais lenta manda no ritmo de todo mundo**. É o preço de "sempre
> sincronizado", e ele é pago em segundos de espera. Se na prática incomodar, o
> conserto não é afrouxar a sincronia — é o host poder expulsar quem está
> segurando a sessão, que é uma decisão social e não técnica.

**Os dois modos de stream existem, como opção da sessão.**

| modo | o que é | quando serve |
|---|---|---|
| **um stream por pessoa** | cada um abre a própria sessão do mesmo arquivo | qualidade por aparelho, e um travar não derruba o do outro |
| **sessão compartilhada** | um transcode só, servido pros dois | mais barato pra máquina, e sincronia mais fácil de garantir |

**Proposto:** o padrão é **um por pessoa** — é o que já funciona hoje sem código
novo, e o compartilhado é a otimização que se liga quando a máquina reclamar. A
escolha fica na criação da sessão, do lado do host.

O barramento de eventos do M3 já entrega mensagem na hora e já foi usado assim
pela locadora e pela conversa. **Proposto:** ele é o transporte, e não se
inventa um segundo canal.

---

## 5. Sequência proposta

**Proposta inteira**, aberta a reordenação. O critério é: primeiro o que está
quebrado, depois o que é barato e aparece, depois o que é fase.

| | o que | por quê aqui |
|---|---|---|
| ~~0~~ | ~~**permissões**~~ | **feito** — R37, §53 |
| **1** | **as capas das sagas** | um `artwork::fetch` que faltou, e conserta dois itens da lista |
| **2** | **os três defeitos pequenos** | ordem por ano · canal Odeon clicável · duração do player |
| **3** | **botão do refresh de sagas** | uma rota que existe e não tem porta |
| **4** | **desafios no "para você"** + **loading da locadora** | baratos, e os dois aparecem todo dia |
| **5** | **gente melhorada** | avatar, o que está vendo, atalho pra conversa |
| **6** | **perfil como Steam** | avatar, capa, vitrines montáveis, URL pública, moldura |
| **7** | **notificação do sistema** | pequena, e depende de decidir o momento de pedir permissão |
| **8** | **animação do rebobinar** | alma, e a dívida mais antiga em aberto |
| **9** | **Watch Party** | fase inteira, e as três perguntas do 4.6 antes |

Os itens 1 a 3 somados são menos trabalho que qualquer uma das oito fases da
primeira rodada. O item 9 é maior que várias delas juntas.

---

## 6. O que continua em aberto

**Nenhuma.** As quatro perguntas foram respondidas na conversa que escreveu este
documento:

| | resposta |
|---|---|
| avatar e capa | **vários prontos pra escolher**, parte deles desbloqueada por conquista. Sem upload |
| Watch Party: quem manda | **o host** |
| Watch Party: um trava | **sempre sincronizado** — todo mundo para |
| Watch Party: stream | **os dois modos**, como opção da sessão |

O que sobra são **propostas**, e elas continuam vetáveis a qualquer momento —
estão marcadas assim no texto. As mais consequentes:

- a arte da saga baixada no próprio job, com varredura única pras 131 (3.1);
- ordenar coleção `provider` por ano, deixando `position` mandar onde existe (3.2);
- o bloco do canal da casa abrir o **cartaz da obra** (3.3);
- a barra do player usar a duração da obra, não a do stream (3.4);
- avatares **desenhados em SVG**, pela régua de zero bytes do §12 (4.1);
- o padrão do Watch Party ser **um stream por pessoa** (4.6).

---

## 7. Dívidas que atravessam isto

Do `DESIGN.md` e do `IDEIAS.md` §7, o que continua de pé:

- **Coleção `manual` não tem dono.** Uma ordem criada por um morador pode ser
  apagada por outro (§53). Hoje é teórico — **zero** coleções manuais — e fechar
  exige migração. Encosta em 3.2 e no perfil.
- **Os clientes Kotlin pararam no M2.** Consomem 10 rotas de ~90 e não conhecem
  nada das oito fases. Maior assimetria do projeto.
- **229 testes e nenhum CI.** Nada os roda automaticamente.
- **A montagem de mídia é gravável** (§22, §42). O conserto é o `:ro` que o
  `docker-compose.yml` documenta como reversível em uma linha.
- **`attach_tag` devolve 500 onde devia ser 404** (§53). Aspereza, não buraco.
