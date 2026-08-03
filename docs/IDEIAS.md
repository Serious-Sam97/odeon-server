# Odeon — o que vem depois

## Sobre este arquivo, e por que ele foi reescrito

A versão anterior deste documento tinha 739 linhas e foi tratada como
especificação durante oito fases de trabalho. Ela não era. Era uma
**interpretação** das ideias de quem decide, escrita numa sessão anterior — e
construir em cima dela produziu um Odeon que ficou longe da visão original.

O erro não foi de onze detalhes. Foi de um:

> A visão é um **servidor de mídia com uma camada social e de jogo por cima** —
> as pessoas jogam junto, conversam, competem, e o acervo é o tabuleiro. O que
> foi construído é um servidor de mídia rigoroso que **argumenta contra** a
> parte social e a parte de jogo.

Este arquivo é reescrito a partir das **anotações cruas** de quem decide e de
uma conversa item a item sobre cada uma delas. A regra de autoria vale para o
documento inteiro:

- **Decidido** — palavra de quem decide, dita explicitamente. Não se mexe sem
  perguntar.
- **Proposto** — sugestão de quem escreve, esperando confirmação. Pode ser
  vetada sem discussão.
- **Medido** — número tirado do acervo real.

Onde não houver marca, é fato de código.

---

## 0. O acervo, hoje

Medido em 03/08/2026:

| | |
|---|---|
| obras | 17.498 — **635 filmes**, 115 séries, o resto episódios e clipes |
| com pôster | 9.018 |
| usuários | **2** |
| histórico | 128 eventos, **2 obras terminadas**, 1 pessoa |
| empréstimos · avaliações | **0 · 0** |

O último número importa mais do que parece: **quase tudo que foi construído nas
últimas fases não tem dado nenhum passando por ele.** A locadora funciona desde
que foi escrita e ninguém pegou uma fita.

---

## 1. As onze ideias, como foram escritas

Este é o texto original, sem edição. Tudo neste documento responde a ele.

1. Guia sobre filmes (Diretores, regiões etc.. pensar mais sobre) no modo experimentação
2. Rotação de filmes no modo locadora? Para mostrar disponibilidade, adicionar um modo onde seja limitado a pessoa alugar
3. Gamificação
4. Classificação e Reviews
5. Desafios
6. Curiosidades sobre filme para a pessoa aprender
7. Conquistas (Filmes da semana, daystreak etc…)
8. Uma mini rede social somente com amigos (e feed?)
9. Adicionar um menu de dvd nos dvds (Uma cena aleatoria do filme rodando de fundo, com musica genérica que combine com o gênero e um menu onde tu da play, escolhe capítulos, coisas interativas com animações etc…)
10. Para VHS ter controle de rebobinar a fita quando devolver na locadora e as pessoas saberem quem devolveu zoado e ter que rebobinar
11. Adicionar estrutura para o sistema adicionar animação de rebobinar a fita, saber quem assistiu, que estado deixou a fita para o próximo uso

---

## 2. Quatro decisões que atravessam tudo

Estas não pertencem a um item só. Elas mudam código que já existe.

### 2.1 Amigos, não "círculo" — **decidido**

A versão anterior inventou um conceito chamado **círculo**: um grupo fechado,
com dono, criado por convite. Ele virou peça de schema e hoje escopa
empréstimo, rotação, notas, feed, convite e o acesso do convidado.

A palavra usada nas anotações é **amigos**. É outra coisa: amizade é entre duas
pessoas, cada um com a sua lista, sem grupo compartilhado.

**Decidido, e feito** (R28, `DESIGN.md` §44): o **estoque da locadora é do
servidor**, não de um grupo — então o empréstimo deixou de precisar de escopo, e
"amigos" passou a existir só onde faz sentido: no social. Apagou uma tabela e
seis pontos de acoplamento em vez de traduzi-los.

**Decidido:** amizade é entre **duas contas que já existem** no Odeon, com
**pedido e aceite**. Não tem nada a ver com o convite: convite dá conta, amizade
é pedida depois. Como não há chave de privacidade (2.2), o aceite **é** o
consentimento.

### 2.2 Transparência entre amigos — **decidido**

Amigo vê o que você está assistindo **agora**, o que largou no meio, o que
terminou, suas notas. Sem chave de privacidade por enquanto.

Isso reverte duas decisões tomadas sem perguntar: o feed atual só mostra o que
foi **terminado** (por privacidade), e a rota que diz quem está assistindo o quê
foi **fechada** na R26, tratada como vazamento. Pela visão, aquilo é feature.

### 2.3 O LLM entra, para conteúdo editorial — **decidido**

Há chave do Groq. O guia e os eventos podem ser escritos por LLM.

Isso derruba, **para este uso**, a regra do `DESIGN.md` §18 que foi aplicada
duas vezes para recusar geração. A regra continua valendo para **fato sobre
filme** — trivia inventada sobre um filme que alguém ama continua sendo pior
que nenhuma.

**Feito assim** (R34): **o sistema manda os fatos, o LLM escreve a costura.** A
lista de filmes, anos e diretores sai do banco; o modelo redige em volta e nunca
é perguntado o que existe no acervo. O que sai leva o selo do modelo, como a
curiosidade da Wikipédia leva o dela.

A chave está posta e o ensaio está sendo escrito. Sem ela, o código continua
omitindo o ensaio em vez de inventar — o estado sem chave foi construído e
exercitado antes de ela existir.

### 2.4 Coletivo e individual têm regra — **decidido**

| coletivo, igual pra todos | individual, por pessoa |
|---|---|
| guia da semana | desafios |
| eventos temáticos | XP, nível, conquistas |

O guia é comunitário **de propósito**: é o que dá assunto em comum. Os desafios
são sorteados por pessoa e mais simples.

---

## 3. As ideias, uma a uma

### 3.1 Guia — a revista que muda

> *"Guia sobre filmes (Diretores, regiões etc.. pensar mais sobre) no modo
> experimentação"*

**Decidido:** um guia **dinâmico**, que muda de temática por dia ou semana, faz
**eventos** de um filme ou saga específica para incentivar as pessoas a
assistir, e usa o acervo para **ensinar história do cinema**. Útil, não
decorativo. **Igual para todo mundo**, para haver assunto em comum.

**O que existe hoje não é isso.** É um índice: cartões por diretor, elenco,
compositor, gênero, década e país, e uma ficha por pessoa cruzando com o seu
histórico. É simples e útil — e é uma enciclopédia, não uma revista.

**Feito** (R34, `DESIGN.md` §50), e a proposta virou o desenho:

- o índice **não morreu**: desceu, e virou a parte de consulta atrás da revista;
- a capa é **o tema da semana** — cinco eixos (gênero, década, país, diretor,
  saga), sorteados do acervo com a **mesma semente semanal da locadora**, então
  ele vira na mesma segunda-feira e é igual pra todo mundo;
- o ensaio é gerado por LLM **sobre fatos do banco**, com selo do modelo. **A
  chave foi posta e ele está escrevendo.** O primeiro texto era honesto e
  inútil — a culpa era dos fatos, não do modelo: com título, ano e diretor não
  há o que dizer além de listar. Entraram país, intervalo de anos, décadas que
  concentram e o total do tema no acervo, e aí ele passou a observar em vez de
  listar. Auditado: **zero filmes, anos ou diretores inventados**;
- o evento amarra com o resto: participar é **terminar durante a janela**, dá XP
  e quatro conquistas novas, e quem participou aparece na capa de todo mundo.

### 3.2 Locadora — estoque escasso

> *"Rotação de filmes no modo locadora? Para mostrar disponibilidade, adicionar
> um modo onde seja limitado a pessoa alugar"*

**Decidido:** a rotação é **estoque de loja**, e escasso. A escala confirmada:

> A locadora tem **~40 caixas na loja inteira** por semana — não 40 por estante.
> O que não está no estoque não existe até o estoque virar. Se alguém aluga, a
> caixa **some da prateleira** e volta quando devolve.

**Decidido:** os números — tamanho do estoque, prazo, quantas por pessoa,
escassez ligada ou não — são **opções no menu do servidor**, para serem
customizados.

**Feito** (R29, `DESIGN.md` §45): a vitrine mostra **40 caixas na loja inteira**,
sorteadas de uma vez — a estante deixou de ser cota e virou endereço, então a
loja muda de forma toda semana e um gênero pode simplesmente não existir numa
segunda-feira. A caixa alugada **some da prateleira** e deixa buraco.

Os quatro números — estoque, prazo, limite por pessoa e a chave de escassez —
são opções na aba `admin`. **Desligada, a chave desliga só o bloqueio:** a loja
continua curta e ninguém barra ninguém.

E a vitrine é **a mesma pra todo mundo**: a semente da rotação perdeu o círculo
na R28, então a caixa da semana é assunto em comum, como o guia (2.4).

### 3.3 Gamificação e conquistas — um sistema só

> *"Gamificação"* · *"Conquistas (Filmes da semana, daystreak etc…)"*

Tratados juntos porque quem decide os tratou juntos.

**Decidido:** algo **parecido com as conquistas da Steam**.

- lista de conquistas **definidas**, e **muitas** — "bem longo";
- em camadas: **de nível, fáceis (dopamina), médias, difíceis, impossíveis**, e
  sobre **sagas e trilogias**;
- os **eventos temáticos do guia** também concedem;
- as pessoas ganham **experiência**, têm **nível**;
- dá pra **comparar com os amigos**;
- **tags e customização** de perfil;
- **quem escreve a lista é quem programa**, não quem decide.

**Decidido e feito** (R32, `DESIGN.md` §48): as conquistas são **retroativas**,
e o XP também. Não custou backfill nenhum — o XP é derivado, então o histórico
que já existia virou medalha sozinho. Com o acervo de hoje isso abriu duas.

**Feito** (R32, `DESIGN.md` §48). O placar e o aviso saíram do produto. No
lugar: **72 conquistas** em seis camadas (fáceis, médias, sagas, difíceis,
impossíveis e marcos de nível), XP derivado, nível em curva triangular, títulos
e tags que só se usam depois de desbloquear, bio livre, vitrine, e a comparação
com os amigos **dentro** do perfil — não numa aba separada, que foi o que fez o
placar ficar escondido.

**Dependência resolvida:** as sagas existem como dado. O job da R32 leu
`belongs_to_collection` dos 548 filmes identificados e criou **133 sagas** —
James Bond com 18 filmes, Sexta-Feira 13 com 10, Harry Potter com 8. Nenhuma
tabela nova: `collection.kind` já aceitava `franchise`.

**Futuro, anotado:** a **temática do site acompanhando o tema da semana do
guia** — o Odeon inteiro fica noir na semana de noir. Fora de escopo agora.

### 3.4 Classificação e reviews

> *"Classificação e Reviews"*

**Decidido:**

- **review de verdade**, com texto;
- **as pessoas podem comentar**;
- **a review mora na ficha do filme**, permanente — é o que alguém lê antes de
  assistir;
- **o feed recebe um post de referência** apontando pra ela;
- toda atividade vira referência no feed: deu nota, terminou o filme, escreveu
  review.

**O que existe hoje:** nota de 1 a 5 com texto opcional na ficha, e a lista de
quem avaliou — desde a R28, **a dos seus amigos**, e não a de quem calhava de
estar no mesmo grupo. Não há comentário, e o feed atual não é um feed de verdade.

**Respondido, e feito** (R33): o comentário existe **nos dois** — uma tabela só,
com alvo polimórfico, e a mesma tela nos dois lugares.

### 3.5 Desafios

> *"Desafios"*

**Decidido:** tarefas **com prazo**, que dão **experiência**. Mais simples que
os temas do guia, e **sorteadas para cada pessoa** — não são iguais pra todos.
A **cadência é escolhida pela pessoa**, entre algumas opções definidas.

**Feito** (R35, `DESIGN.md` §51). Três por janela — um fácil, um de tema e um
que empurra pra fora do seu gosto —, sorteados **por pessoa**, com cadência
escolhida entre todo dia, 3 em 3 dias e toda semana. Dão XP e quatro conquistas.
**Falhar não custa nada:** a janela fecha e outro é sorteado.

### 3.6 Curiosidades — **feito e aprovado**

> *"Curiosidades sobre filme para a pessoa aprender"*

Na ficha do filme: prêmios, orçamento × bilheteria, locações, um parágrafo da
Wikipédia com crédito e link, e coisas tiradas do próprio acervo (*"de Martin
Campbell você também tem GoldenEye"*, *"você parou faltando 39 minutos"*).

Ver `DESIGN.md` §32 e §33. **Fica como está.**

### 3.7 Menu de DVD — refazer por cima

> *"Uma cena aleatoria do filme rodando de fundo, com musica genérica que
> combine com o gênero e um menu onde tu da play, escolhe capítulos, coisas
> interativas com animações etc…"*

**Decidido:** a ideia está lá, mas falta **muito**. Precisa ser mais dinâmico,
ter mais efeitos, ser **realmente um menu de DVD clássico, com alma**.

A referência é a **edição especial de 2004**:

- **vinheta animada** antes de o menu aparecer;
- **vídeo rodando dentro dos itens** do menu;
- **transição própria por submenu** — a câmera "viaja" até a tela de capítulos;
- **trilha em loop costurado**;
- e o **estilo sai da temática do filme** — comédia e terror não ganham o mesmo
  menu.

**Também decidido, e já estava na anotação original:** a cena de fundo é
**aleatória**. Hoje ela é determinística (sempre um quinto da duração).

**Feito** (R31, `DESIGN.md` §47): os dois bugs corrigidos — o clima do menu sai
da **mesma ordem de reivindicação da locadora**, então o filme que mora na
estante de terror abre um menu de terror, e são doze climas em vez de três
variantes; a grade rola. A cena de fundo virou sorteada, a grade virou
**capítulos numerados**, e a experiência 2004 está lá: vinheta pulável, vídeo
rodando dentro dos itens, a câmera viajando até o submenu, trilha costurada e o
estilo saindo do clima do filme.

**Sobre capítulos — respondido.** Medido de novo e o número é pior: **3 filmes
de 635** têm capítulo no arquivo, e **nenhum** tem nome. Mesmo assim a grade se
chama **capítulos**, numerada: o Odeon não diz que o arquivo declarou — ele
divide o filme em capítulos, que é o que faz. A legenda continua dizendo de onde
veio o corte.

**O esqueleto serve** — sessão HLS com offset, sintetizador, máquina de estados
— mas o trabalho é grande, não é acabamento.

### 3.8 A rede social

> *"Uma mini rede social somente com amigos (e feed?)"*

**Decidido:** uma **aba separada**, que talvez venha a ser algo separado do
Odeon. Nela:

- **feed dos seus amigos** — o que fizeram e o que **estão fazendo**;
- **adicionar pessoas** e **pesquisar** por elas;
- **duas listas de presença**: quem está online **no servidor** e quem está
  online **entre os seus amigos**;
- **mensagem direta**;
- **perfil customizável** (liga com as tags e a customização de 3.3);
- as pessoas **postam** — e além disso, toda atividade vira **referência** no
  feed.

**Decidido sobre visibilidade:** **tudo aberto entre amigos.** Sem chave de
privacidade por ora.

**Feito** (R33, `DESIGN.md` §49). A aba subiu pra primeiro nível, com três
salas: **mural** (feed, caixa de post e presença), **conversas** e **gente**
(amigos, pedidos e busca). O feed deixou de esconder o que não foi terminado —
mostra o que está sendo assistido **agora** e o que foi largado no meio. As
pessoas postam e comentam, e o comentário é o mesmo nos posts e nas reviews.
Presença em duas listas, e mensagem direta entre amigos.

### 3.9 A fita, e quem a devolveu zoado

> *"Para VHS ter controle de rebobinar a fita quando devolver na locadora e as
> pessoas saberem quem devolveu zoado e ter que rebobinar"*
> *"Adicionar estrutura para o sistema adicionar animação de rebobinar a fita,
> saber quem assistiu, que estado deixou a fita para o próximo uso"*

São duas anotações sobre a mesma coisa — e juntas são o item mais detalhado da
lista, o que diz quanto ele importa.

**Decidido, cena a cena:**

- você **descobre quando põe pra tocar** — não na estante, não antes;
- rebobinar **leva alguns segundos de verdade**, pra simular o que se passava —
  mas sem ser massante;
- existe **log de quem faz certo e quem devolve zoado**, e as pessoas sabem.

**Decidido, contra a proposta:** rebobinar **é obrigatório**. Não há "dar play
daqui" — a fita de outra pessoa se rebobina antes, e os segundos que isso custa
são o preço do descuido dela. (Inferido, e passível de aperto: se foi **você**
que deixou no meio, não há obrigação — isso é a sua sessão continuando.)

**Feito** (R30, `DESIGN.md` §46). A fita virou **objeto próprio**, separado do
`playback_state` de qualquer pessoa — e foi isso que dissolveu a recusa do §35:
rebobinar deixou de apagar o "continuar de onde parou" de alguém. A fita anda
enquanto você assiste, o estado só aparece no play, e o log guarda dois nomes
(quem teve o trabalho e quem deixou assim), que viram reputação no balcão.

**Fica devendo:** a animação é um ponteiro regressivo e um carretel andando pra
trás. Falta o objeto girando, o ruído, o tranco no fim — mesmo tipo de trabalho
da fase 4.

---

## 4. O que precisa ser desfeito

Nem tudo que existe é base. Isto aqui atrapalha:

| o que | onde | por quê |
|---|---|---|
| ~~**o círculo**~~ | ~~migração `0021`~~ | **feito na R28** (`DESIGN.md` §44): a tabela morreu, o estoque virou do servidor, e amizade entrou no lugar |
| ~~**o feed só do que terminou**~~ | ~~`feed.rs`~~ | **feito na R33** (§49): mostra o que está rodando agora e o que foi largado |
| ~~**a presença fechada**~~ | ~~`playback.rs`~~ | **feito na R33**, por outra fonte: `last_seen_at` e o heartbeat, não o transcode |
| ~~**o placar com aviso**~~ | ~~`placar.rs`, `Placar.tsx`~~ | **feito na R32** (`DESIGN.md` §48): apagado, e o perfil entrou no lugar |
| **a retrospectiva no lugar de conquistas** | `retrospectiva.rs` | substituiu o que foi pedido por outra coisa. Pode sobreviver como tela de perfil |

**Revisado na R28:** o **convite** passou a ser do servidor e o papel `guest`
ficou — ele continua só assistindo o que pegou emprestado, e a regra só perdeu o
JOIN com o círculo. O token de mídia curto (`§43`) é bom independente disso e
fica. A **presença fechada** continua fechada, e cai na fase 6.

---

## 5. Sequência proposta

**Proposta inteira**, aberta a reordenação. O critério é: primeiro o que
desamarra, depois o que dá alma, depois o que mede.

| | o que | por quê aqui |
|---|---|---|
| ~~**1**~~ | ~~**Amigos no lugar do círculo**~~ | **feito** — `DESIGN.md` §44 |
| ~~**2**~~ | ~~**Estoque escasso + opções no servidor**~~ | **feito** — `DESIGN.md` §45 |
| ~~**3**~~ | ~~**A fita zoada**~~ | **feito** — `DESIGN.md` §46 |
| ~~**4**~~ | ~~**Menu de DVD, de verdade**~~ | **feito** — `DESIGN.md` §47 |
| ~~**5**~~ | ~~**XP, nível e conquistas**~~ | **feito** — `DESIGN.md` §48 |
| ~~**6**~~ | ~~**A rede social**~~ | **feito** — `DESIGN.md` §49 |
| ~~**7**~~ | ~~**O guia dinâmico**~~ | **feito** — `DESIGN.md` §50 |
| ~~**8**~~ | ~~**Desafios**~~ | **feito** — `DESIGN.md` §51 |

**As oito estão feitas.** A observação sobre tamanho continua verdadeira — era
mesmo maior que as oito fases anteriores somadas —, e ficou registrado em
`DESIGN.md` §44 a §51 o que cada uma decidiu e por quê.

**O que segue em aberto, e não está disfarçado:**

- ~~**não há `GROQ_API_KEY`**~~ — **posta**, e o ensaio do guia (§50) está
  sendo escrito. O texto melhorou quando os fatos melhoraram, e a ressalva da
  2.3 foi auditada: nada inventado;
- **os clientes Kotlin pararam no M2** e não conhecem nada do que foi construído
  hoje — continua sendo a maior assimetria do projeto;
- **229 testes e nenhum CI.** XP, conquista, desafio e evento são regra de
  negócio de verdade, e é onde um teste quebrado importa mais que um screenshot.

---

## 6. O que continua em aberto

**As cinco foram respondidas** — todas por quem decide, nenhuma por quem
programa. Ficam registradas com a resposta.

1. ~~**Capítulos ou cenas** no menu de DVD~~ — **respondido: capítulos.**
   Feito na R31.
2. ~~**Comentário existe onde**~~ — **respondido: nos dois.** Uma tabela só,
   com alvo polimórfico. Feito na R33.
3. ~~**Conquistas retroativas** ao ligar~~ — **respondido: sim, com XP.**
   Feito na R32.
4. ~~**Rebobinar é obrigatório** ou opcional~~ — **respondido: obrigatório.**
   Feito na R30.
5. ~~**R26/R27 — convidado e convite**~~ — **respondido:** o convite é do
   servidor, não cria amizade nenhuma, e o `guest` fica. Feito na R28.

---

## 7. Dívidas herdadas que encostam nisto

Do `DESIGN.md`, o que vira pré-requisito ou risco:

- ~~**Sagas de filme não existem como dado.**~~ **Pago na R32:** 133 sagas
  materializadas de 548 filmes. Os eventos do guia (fase 7) já têm em que se
  apoiar.
- **A montagem de mídia é gravável** (§22, §42). Nenhuma rota que um convidado
  alcance escreve em disco, e o conserto é o `:ro` que o `docker-compose.yml`
  documenta como reversível em uma linha.
- **Os clientes Kotlin pararam no M2.** Consomem 10 rotas de ~90 e não conhecem
  curadoria, ao vivo, emissora, locadora, guia nem nada deste documento. Não é
  pré-requisito de nada, e é a maior assimetria do projeto.
- **229 testes e nenhum CI.** Nada os roda automaticamente. XP, conquista,
  desafio e evento são regra de negócio de verdade — é onde um teste quebrado
  importa mais que um screenshot.
- **Nada commitado.** Oito fases, oito migrações e oito seções de documento no
  working tree da `main`.
