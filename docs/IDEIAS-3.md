# Odeon — terceira rodada

## Sobre este arquivo

O `IDEIAS.md` é o registro fechado da rodada 1; o `IDEIAS-2.md` é o da rodada 2,
entregue inteira nas fases R38 a R46 (`DESIGN.md` §54 a §62). Os dois não se
mexem.

Este é o começo da terceira, com a mesma regra de autoria:

- **Decidido** — palavra de quem decide, dita explicitamente. Não se mexe sem
  perguntar.
- **Proposto** — sugestão de quem escreve, esperando confirmação. Pode ser
  vetada sem discussão.
- **Medido** — número tirado do acervo ou do código, hoje.

Onde não houver marca, é fato de código.

E a regra de trabalho das duas rodadas anteriores continua valendo: **quando a
ideia parecer "errada" pela régua de engenharia, perguntar ou fazer o que foi
pedido — nunca entregar a versão sóbria por conta própria.**

---

## 0. O estado, medido em 03/08/2026

| | |
|---|---|
| usuários | 3 — `sam` (admin), `rudney` e `gabriel` |
| obras · filmes · séries | 17.498 · 635 · 115 |
| coleções | 709 · **133 sagas, todas com capa local** (era zero antes da R38) |
| migrações · testes · CI | 37 · **241** · nenhum |
| tamanho do código | `backend/src` 1,2M · `web/src` 852K · `clients` 272K |

Três números que decidem coisas neste documento:

> **Nenhum segredo é versionado.** `.env` está no `.gitignore` desde o primeiro
> commit, o repositório carrega só o `.env.example`, e não há chave em texto em
> nenhum arquivo rastreado. É o que torna publicar o servidor uma decisão de
> vontade, e não uma limpeza de histórico.

> ~~**Sete listas horizontais** usam `overflow-x: auto` hoje: `.fileira` (a
> estante da locadora), `.fileira-canais` (ao vivo), `.guia-fileira`, `.pv-fila`
> (para você), `.cartaz-fila`, `.credit-group` e `.credit-people`.~~
>
> **Corrigido na R48 (§64): são seis, e duas da lista acima não existem.**
> `.credit-people` e `.credit-group` são **CSS morto** — nenhum elemento carrega
> essas classes, e a segunda nem `overflow-x` tem. Faltava `.abas`, que rola em
> janela estreita pelo `@media` de 900px. A conta certa: `.fileira`,
> `.fileira-canais`, `.guia-fileira`, `.pv-fila`, `.cartaz-fila` e `.abas` — e
> não são seis **elementos**: a locadora e a wiki nascem dentro de um `map`.

> **`acesso::pode_assistir` já existe desde a R26** e já barra bytes de quem não
> tem empréstimo em aberto — só que hoje ela vale **só pra convidado**: morador
> sai antes, pelo `e_morador`.

---

## 1. As anotações, como foram escritas

Texto original, sem edição. Tudo neste documento responde a ele.

```
* Fazer a separação de repos do client para o server
* Deixar a seleção de pastas melhor
* Adicionar rota para cada tab (Assim o reload te deixa na exata tab)
* Todo bloco que tiver uma lista na horizontal deve ter um grab and move com
  mouse para facilitar usabilidade
* Para dar play nos filmes é necessário pegar emprestado (SOMENTE MODO LOCADORA)
* Colocar foto de perfil no menu header junto ao nivel
```

---

## 2. Um item já estava feito

> *"Adicionar rota para cada tab (Assim o reload te deixa na exata tab)"*

**Feito na R43** (§59), e a anotação é anterior a ela. As onze telas têm
endereço em português — `/biblioteca`, `/colecoes`, `/locadora`, `/guia`,
`/ao-vivo`, `/mural`, `/perfil`, `/revisao`, `/pastas`, `/admin`, e a raiz é o
"para você" —, mais `/p/<nome>` pro perfil de alguém. **Conferido em navegador:**
entrar direto em `/locadora` abre a locadora com a aba certa acesa, e o botão
voltar devolve a tela anterior.

**Decidido: o item sai da lista.**

O que **não** está na URL é o estado de dentro das telas: a sala do mural, os
filtros da biblioteca, a ficha aberta, a fila de revisão. Isso é a dívida que o
§59 registrou, e não foi o que a anotação pediu — fica anotada no §6 deste
documento, sem entrar na sequência.

---

## 3. Separar o repositório do servidor

> *"Fazer a separação de repos do client para o server"*

**Decidido:** o objetivo é **publicar o servidor sozinho** — o backend vira
repositório próprio, e o cliente segue no seu.

### O que a medição diz

O laço entre as partes é **um arquivo**: o `docker-compose.yml`. Não há tipo
compartilhado, não há código gerado, não há import cruzado. `web/src/api.ts`
descreve as respostas do servidor em TypeScript escrito à mão, e é a única
"cópia" de contrato que existe — e ela já é uma cópia hoje.

E o que costuma travar uma publicação não trava esta: **não há segredo no
histórico**. `.env` está ignorado desde o commit inicial.

### O que precisa ser decidido, e ainda não foi

**Onde mora o `DESIGN.md`.** Ele tem 7.200 linhas e é a alma do projeto — e fala
das duas metades: a estante 3D da locadora e o `artwork::fetch` do servidor
estão nas mesmas seções. Rachá-lo em dois documentos perderia exatamente o que
ele tem de melhor, que é a costura.

**Proposto:** o `DESIGN.md` vai **inteiro** pro repositório do servidor, e o do
cliente aponta pra ele. Publicar o servidor com a documentação que explica por
que cada escolha foi feita é o que dá valor à publicação; um servidor sem ela é
mais um media server no GitHub.

**Proposto:** o `docker-compose.yml` racha em dois. O do servidor sobe API e
banco e é o que um estranho roda; o do cliente sobe o web apontando pra uma API
que já existe. Hoje um arquivo faz as duas coisas porque as duas moravam juntas.

**Proposto:** os clientes Kotlin (`clients/`, 272K, parados no M2) vão com o
**cliente**, não com o servidor. São consumidores da API, como o web.

**Decidido, e entregue na R51 (§67):** os dois nascem **públicos**, sob
**AGPL-3.0** — a licença que corresponde a um software usado *através* da rede,
onde a GPL comum nunca dispararia a obrigação de publicar. Nenhuma das duas era
técnica, e nenhuma foi minha.

---

## 4. A seleção de pastas

> *"Deixar a seleção de pastas melhor"*

Hoje são **duas telas que não se falam**:

| | o que faz | onde |
|---|---|---|
| `pastas` | cadastra a raiz de uma biblioteca, digitando ou navegando | `Libraries.tsx` |
| `revisão › pastas` | decide obra por obra, pasta a pasta | `Scopes.tsx` |

E uma rota `/api/browse` que lista o disco a partir das raízes montadas — só pra
administrador, e resolvendo o caminho contra `ODEON_MEDIA_ROOTS` pra ninguém
passear pelo sistema de arquivos.

**Falta o essencial**, e é o que a anotação pede: a tela não diz nada sobre a
pasta antes de você escolhê-la. Quantos vídeos tem dentro, se já está numa
biblioteca, o que ela parece ser.

**Proposto:** navegar mostrando **o que a pasta é** — contagem de vídeos, se já
está coberta por uma biblioteca, e um palpite do tipo (filme, série, mistura)
tirado do próprio `scanner::guess`, que já sabe ler nome de arquivo. Escolher
uma pasta sem saber o que tem dentro é escolher no escuro, e é isso que a tela
faz hoje.

**Proposto:** as duas telas continuam duas. Elas fazem coisas diferentes —
montar o acervo e corrigir o acervo — e juntá-las era o que a "experimentação"
fazia com o menu, que a R36 desfez.

**Entregue na R49 (§65), e com duas coisas que este texto não previa:**

- **Metade do disco não tinha caminho até esta tela.** O navegador nasce na
  primeira raiz e o "subir" para nela: com `/media` e `/media2` montados, nada
  levava a `/media2`. O campo `roots` já vinha na resposta desde o primeiro dia,
  sem ninguém desenhar.
- **A cobertura não virou um selo, virou um botão que some.** `create_library`
  já recusava biblioteca aninhada nas duas direções — a tela oferecia e comia um
  400, que é o §53.

---

## 5. Arrastar as listas horizontais

> *"Todo bloco que tiver uma lista na horizontal deve ter um grab and move com
> mouse para facilitar usabilidade"*

**Medido: sete listas** (§0). Nenhuma tem arrasto; todas dependem de roda do
mouse com Shift, de trackpad, ou da barra que o CSS esconde.

**Proposto:** um comportamento só, compartilhado — um gancho que qualquer
fileira usa —, e não sete implementações. É a mesma decisão que fez o
`ouvirEventos` da R46 existir depois de quatro telas escreverem as mesmas seis
linhas.

**Proposto, e é a parte que merece cuidado:** a estante da locadora tem caixas
com `setPointerCapture` e giro 3D (§35), e a R39 já apanhou disso — arrastar
para rolar **não pode** virar "peguei a caixa". A separação é a de sempre: um
movimento acima de alguns pixels é rolagem, abaixo é clique. É a mesma conta que
o `arrastou()` da caixa na mão já faz.

---

## 6. Pegar emprestado para dar play

> *"Para dar play nos filmes é necessário pegar emprestado (SOMENTE MODO
> LOCADORA)"*

**Decidido: a regra vale quando a escassez está ligada.** Não é uma segunda
chave no painel — é parte do pacote "locadora de verdade" que a R29 já criou.

Isso é melhor do que uma opção nova por um motivo que vale escrever: a escassez
já significa *"uma cópia por caixa, e quem pegou tirou da prateleira"*. Exigir o
empréstimo pra assistir é a **consequência** disso, não uma regra ao lado. Com a
escassez desligada, a locadora é um tema; com ela ligada, é o mecanismo.

### Onde ela mora, e por que é uma linha

`acesso::pode_assistir` já é o guarda: `/api/stream`, o plano de reprodução, a
sessão de transcode e o menu do disco passam por ela. O que muda é o `e_morador`
deixar de ser um cheque em branco quando a escassez está ligada.

**Decidido, e entregue na R50 (§66):** vale **para todo mundo, inclusive
administrador**. Uma regra com porta dos fundos pro dono não é uma regra — é um
tema. E ela é reversível numa chave, como a escassez sempre foi — conferido: a
opção é lida na própria consulta, então desligar e religar vale no clique
seguinte, sem reiniciar nada.

### O que dá trabalho não é a regra, é a tela

**Medido: nove pontos de play** hoje — biblioteca, coleções, para você (cinco
entradas), ficha, menu de DVD, locadora. Com a chave ligada, todos eles precisam
parar de oferecer o que a validação vai recusar (§53) e **dizer onde se pega**:
o botão vira "pegar na locadora" e leva pra caixa.

Um 403 na cara de quem clicou em ▸ assistir seria o §8b: o produto oferecendo o
que ele sabe que vai negar.

---

## 7. O rosto no cabeçalho

> *"Colocar foto de perfil no menu header junto ao nivel"*

A R43 guarda o rosto escolhido e a barra já desenha o **anel de nível** em volta
do seu nome (§52). É juntar os dois: o rosto dentro do anel.

**Proposto:** quem não escolheu rosto continua com a marca derivada do nome
(R42) — ela é o padrão, e o cabeçalho não pode ter buraco.

---

## 8. Sequência proposta

**Proposta inteira**, aberta a reordenação. O critério é o mesmo das rodadas
anteriores: primeiro o barato que aparece todo dia, depois o que muda
comportamento, e a mudança estrutural por último — porque ela é a única que não
dá pra desfazer com um `git revert` tranquilo.

| | o que | por quê aqui |
|---|---|---|
| ~~0~~ | ~~**rota por tab**~~ | **feito** — R43, §59 |
| ~~1~~ | ~~**o rosto no cabeçalho**~~ | **feito** — R47, §63 |
| ~~2~~ | ~~**arrastar as listas**~~ | **feito** — R48, §64 (seis lugares, não sete) |
| ~~3~~ | ~~**a seleção de pastas**~~ | **feito** — R49, §65 |
| ~~4~~ | ~~**pegar emprestado pra assistir**~~ | **feito** — R50, §66 |
| ~~5~~ | ~~**separar o repositório**~~ | **feito** — R51, §67 (preparado, não publicado) |

---

## 9. O que continua em aberto

**Decidido, depois que este documento foi escrito: os dois repositórios vão ser
públicos.** Isso responde metade da primeira pergunta do §3 e muda o peso da
outra metade — a licença deixa de ser papelada e passa a ser o que diz o que um
estranho pode fazer com 7.400 linhas de `DESIGN.md` e um servidor inteiro. Ela
**continua em aberto, e é sua**: não escolho uma sem perguntar.

| | |
|---|---|
| ~~público ou privado~~ | **decidido: públicos, os dois** |
| ~~a licença~~ | **decidida: AGPL-3.0** — a brecha que ela fecha é a do servidor servido pela rede (R51, §67) |
| ~~o `DESIGN.md`~~ | **decidido: inteiro pro servidor**, e o cliente aponta pra ele |

**Não sobrou nenhuma.** O que falta é um gesto, não uma decisão: os dois
repositórios estão montados em `odeon-split/`, sem commit e sem push.

---

## 10. Dívidas que atravessam isto

Do `DESIGN.md`, do `IDEIAS.md` §7 e do `IDEIAS-2.md` §7 — o que continua de pé:

- **241 testes e nenhum CI.** Nada os roda automaticamente, e três das nove
  fases da rodada 2 acharam defeitos que estavam calados havia meses. É a dívida
  que mais paga juros.
- **O estado de dentro das telas não está na URL** (§59): sala do mural, filtros
  da biblioteca, ficha aberta, fila de revisão. O router já está no lugar.
- **Os clientes Kotlin pararam no M2.** Consomem 10 rotas de ~90.
- **A montagem de mídia é gravável** (§22, §42) — o `:ro` é uma linha.
- **`attach_tag` devolve 500 onde devia ser 404** (§53).
- **Coleção `manual` não tem dono** (§53). Ainda zero delas no servidor.
- **Do que a rodada 2 deixou dito:** o modo compartilhado do assistir junto
  nunca foi exercido (§62), o encadeamento varredura → sagas não rodou de ponta
  a ponta (§56), e duas sagas guardam `{"poster": null}` porque o TMDB não tem
  arte delas (§54).
