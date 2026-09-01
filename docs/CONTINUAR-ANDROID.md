# O app Android continua noutro computador

Escrito em **04/08/2026**, no `serious-server`.

Este é o repositório do **servidor**. O app Android mora no outro
(`Serious-Sam97/odeon-client`, em `android/`), e a partir daqui ele é escrito e
rodado noutro computador.

> **O documento de verdade é o de lá:**
> https://github.com/Serious-Sam97/odeon-client/blob/main/docs/CONTINUAR-ANDROID.md
>
> Este aqui existe pra quem chegar por este repositório saber onde procurar, e
> pra registrar o que **o servidor** precisa continuar oferecendo.

---

## Por que dividiu

O app Android foi escrito até a fase 1 (entrar e ver a biblioteca) e **nunca foi
visto rodando**: o emulador segfalta neste computador, em sete configurações
diferentes. O diagnóstico completo está em `android/README.md` do outro
repositório.

O outro computador já tem emulador funcionando e está na mesma tailnet, e passa a
ser o dono do app Android.

## ⚠️ E este repositório continua sendo trabalho do `serious-server`

O corte é por pasta, e é curto de dizer:

| | o outro computador | `serious-server` |
|---|---|---|
| `odeon-client/android/` | ✅ inteiro | não toca |
| `odeon-client/web/` e `clients/` | ❌ | ✅ |
| **`odeon-server/` (aqui)** | ❌ **nada, nunca** | ✅ |
| o banco, a identificação, as migrações | ❌ | ✅ |

**O outro computador não abre este repositório.** Ele consome a API por HTTP e
mais nada — como qualquer cliente, e como a separação dos repositórios já
determinava (§67).

Quando o app precisar de uma mudança de servidor — e a espec já prevê duas, o
`ODEON_ALLOWED_ORIGINS` pro Cast (§4c) e talvez a porta padrão —, **o caminho
passa pelo dono**: o outro computador avisa o que falta, ele traz o pedido pra
cá, e quem mexe é quem está na máquina, olhando o que ela está fazendo naquele
momento.

O formato do pedido está no documento do cliente: o que precisa, por quê, o que
quebra sem isso, e **o que já foi tentado do lado do cliente**. Esse último item
é o que evita mudar o servidor à toa — a própria espec tem o exemplo: com Cast, a
negociação de reprodução passa a falar de outro aparelho, e o conserto é o app
mandar o perfil do Chromecast em `/api/playback/{id}/plan`, que já aceita a lista
como parâmetro. Nenhuma linha daqui.

Não é burocracia. As migrações são embutidas no binário em tempo de compilação
(`sqlx::migrate!`), este servidor está no ar servindo três pessoas de verdade, e
a identificação leva ~1h e morre se o processo reiniciar. "Só ajustar uma linha"
daqui é a forma mais curta de derrubar tudo isso.

---

## ⚠️ O que este servidor precisa continuar fazendo

O app do outro computador só existe enquanto este servidor estiver alcançável.

**1. Ficar de pé e ouvindo em todas as interfaces.**

```bash
docker port odeon-api
# 8080/tcp -> 0.0.0.0:8085     ← é esta que a tailnet alcança
# 8443/tcp -> 0.0.0.0:8443     ← mapeada, mas o TLS está desligado
```

**2. O Tailscale ligado.** O endereço que o outro computador usa é o **IPv4 da
tailnet**, e não o nome da máquina — MagicDNS não resolve dentro de um emulador
Android:

```
100.77.253.18:8085
```

Conferir daqui, antes de culpar o app:

```bash
curl -s http://100.77.253.18:8085/api/auth/status   # {"needs_setup":false}
curl -s http://100.77.253.18:8085/api/health        # {"db":true,"status":"ok",…}
```

**3. Avisar quando um contrato mudar.** Não há tipo compartilhado entre servidor
e cliente — `web/src/api.ts`, o `shared` do KMP e agora
`android/…/dados/Modelos.kt` são **três cópias à mão** do mesmo contrato. É a
dívida que a separação dos repositórios comprou, e está registrada no §67.

Uma rota que muda de forma aqui não avisa ninguém lá. Os modelos do Android
foram conferidos campo a campo contra o Rust em 04/08/2026; se `LibraryEntry` ou
`User` mudarem, alguém tem que ir lá.

---

## As cinco coisas que o app consome hoje

De **113 rotas**, a fase 1 fala cinco:

| rota | o que o app faz com ela |
|---|---|
| `GET /api/auth/status` | confirma que ali **é** um Odeon antes de mandar senha |
| `POST /api/auth/login` | `{username, password, device_label}` → `{token, user}` |
| `GET /api/auth/me` | quem entrou |
| `POST /api/auth/media-token` | o token curto que abre bytes |
| `GET /api/library` | a biblioteca, agrupada por série, paginada |

`GET /api/library` e não `/api/works`: o primeiro agrupa, o segundo devolveria os
episódios um a um — e a web já concluiu o que isso é, "listagem de arquivo e não
biblioteca".

O `device_label` do login importa: é o rótulo que aparece na tela de aparelhos do
admin. O app manda fabricante + modelo.

### Aplicar uma pasta: a prévia responde, a escrita vira job

Escrito em **25/08/2026** — **R81**. Isto MUDA o contrato de
`POST /api/scopes/identify`: quem chamava e lia o resumo na resposta precisa
passar a acompanhar um job.

**Por quê.** Em 25/08 esta chamada morreu no ar aplicando o Popeye: o escopo foi
gravado às 05:37:57, os episódios foram confirmados um por segundo até 05:40:01,
e aí o túnel cortou (`Incoming request ended abruptly: context canceled`). São
124 segundos, e a Cloudflare corta resposta de origem por volta dos 100. O
navegador mostrou `TypeError: NetworkError when attempting to fetch resource` —
que não diz nada. Dos 215 episódios, 149 tinham sido processados e 66 nunca
foram alcançados: o axum cancela o handler junto com a conexão.

Não dava pra empurrar o teto. A 1,2 obra por segundo, 100 segundos comportam
~120 arquivos, e a fila tem pastas de 388, 331 e 313.

| o que você quer | como fica |
|---|---|
| **ver o que vai acontecer** (`dry_run: true`) | síncrono, responde na hora |
| **aplicar** (`dry_run: false`) | abre job, responde na hora com o `job_id` |

```jsonc
// A PRÉVIA — não mudou nada. 0,28 s medidos em 103 obras.
POST /api/scopes/identify   { …, "dry_run": true }
→ 200 { "afetados": 103, "confirmariam": 53, "ficariam_em_revisao": 50,
        "chamadas_de_temporada": 1, "preview": [ …até 25 itens… ] }

// A ESCRITA — mudou. Responde em ~30 ms, não espera o trabalho.
POST /api/scopes/identify   { …, "dry_run": false }
→ 200 { "started": true, "job_id": "…", "pasta": "…",
        "acompanhe": "/api/jobs/<id>" }
→ 200 { "started": false, "reason": "já há uma pasta sendo aplicada — …" }

// O ACOMPANHAMENTO — rota nova.
GET  /api/jobs/{id}
→ 200 { "kind": "scope_apply", "state": "running",
        "done": 60, "total": 103,
        "progress": { "pasta": "…", "aplicados": 16, "arquivo": "Popeye - S01E067…" } }
→ 404                                  // id que não existe
POST /api/jobs/{id}/cancel
→ 200 { "ok": true, "detalhe": "vai parar no próximo item — …" }
```

`state` termina em `succeeded`, `cancelled` ou `failed`. Em qualquer um dos três
o resumo final está em `progress`, com os campos que a resposta síncrona
devolvia antes — mais três novos:

* **`afetados`** — quantos arquivos a pasta tinha.
* **`processados`** — quantos foram VISTOS. Existe por causa do Popeye:
  `aplicados: 112` sozinho não deixa ninguém saber se faltaram 66 arquivos ou se
  eram só esses.
* **`cancelado`** — se parou por pedido.

⚠️ **Só uma pasta por vez** (`job_one_active_per_kind`). A segunda chamada
devolve `started: false` — é recusa, não fila, e a tela deve dizer isso.

⚠️ **O cancelamento é cooperativo.** Para entre um arquivo e outro, nunca no
meio de um — cancelar entre gravar o candidato e aplicar a obra deixaria estado
pela metade. O que já gravou, fica: não há transação envolvendo a pasta inteira,
e é por isso que os 112 episódios do Popeye sobreviveram ao corte.

Medido depois da mudança, na mesma pasta: resposta em **31 ms**, job completo em
**32 s**, 80/80, com progresso visível e cancelamento funcionando.

### Buscar no disco: os três gestos

Escrito em **20/08/2026**, com o formato junto — a lição do token de arte.

| gesto | chamada |
|---|---|
| buscar e identificar **os filmes** | `POST /api/scan?tipo=filme&then=match` |
| buscar e identificar **as séries** | `POST /api/scan?tipo=serie&then=match` |
| **os dois**, e o resto | `POST /api/scan?then=match` |

```jsonc
POST /api/scan?tipo=filme&then=match     // Authorization: Bearer <sessão de admin>
→ 200 { "started": true, "job_id": "…", "then": "match" }
→ 200 { "started": false, "reason": "scan já em andamento" }   // trava única
```

O andamento sai em `GET /api/scan/status` e `GET /api/jobs`; o fim vira evento
no barramento (`ScanFinished`, depois `MatchFinished`).

* **`tipo`** corta por `library.default_kind`. Aceita `filme`/`filmes`/`movie`
  e `serie`/`série`/`series`/`episode`. **Ausente ou desconhecido varre tudo** —
  um erro de digitação não pode varrer metade do acervo em silêncio.
* **`then=match`** encadeia a identificação, e depois dela as sagas. Sem ele a
  busca só descobre arquivo. O escopo do `tipo` **atravessa as duas etapas**:
  "buscar os filmes" não identifica a série que estava na fila por acaso.
* ⚠️ **Um `tipo` de cada vez não marca o outro como sumido.** A varredura de
  filmes não enxerga as bibliotecas de série, então não conclui que elas
  desapareceram. Conferido: `sumidos = 0` nas duas passadas.

⚠️ **A trava é uma só pro servidor inteiro.** Duas buscas simultâneas devolvem
`started: false` na segunda — não é fila, é recusa, e o cliente deve dizer isso
em vez de fingir que enfileirou.

### Os três tokens, e qual serve pra quê

Escrito em **20/08/2026**, porque `grep -r artwork-token` dava zero em todos os
clientes e a entrega de 17/08 estava no ar sem ninguém saber pedir. A lição de
processo é dos dois lados: **entrega sem formato escrito é entrega que não
existe.**

| token | como se pede | dura | onde vale |
|---|---|---|---|
| **sessão** | `POST /api/auth/login` → `{token, user}` | 90 dias | header `Authorization: Bearer` ou cookie `odeon_session`. **Não** funciona em `?token=` |
| **mídia** | `POST /api/auth/media-token` → `{token, horas}` | 8 h | `?token=` em `/api/stream/`, `/api/hls/`, `/artwork/`, `/scrub/`, `/api/events`, legendas |
| **arte** | `POST /api/auth/artwork-token` → `{token, dias}` | **365 dias** | `?token=` em **`/artwork/` e mais nada** |

As três emissões são autenticadas do jeito normal — header ou cookie. O token é
o que a rota **devolve**, nunca o que a abre.

```jsonc
POST /api/auth/artwork-token          // Authorization: Bearer <sessão>
→ 200 { "token": "…64 hex…", "dias": 365 }

GET /artwork/<caminho>?token=<o de arte>   → 200
GET /api/stream/<id>?token=<o de arte>     → 401   // de propósito
```

**Quando usar o de arte, e não o de mídia:** sempre que a URL sair do processo
do app e for buscada por outro. São dois casos conhecidos, e os dois já doeram:

* a **fileira da home da Google TV** — o que se entrega ao `TvProvider` é uma
  `Uri`, e quem a baixa é o launcher, dias depois, com o Odeon fechado;
* a **capa da notificação de mídia** — quem baixa é o system UI, fora do
  processo, e ele não tem como pedir um token novo quando o velho vence.

Nos dois, um token de mídia dá o sintoma que o cliente relatou —
`NotificationProvider: Failed to load bitmap` e retângulo vazio na home —
porque ele vence em 8 horas.

⚠️ **Não aposenta os anteriores.** O de mídia é podado por aparelho (R61); o de
arte guarda os **cinco mais novos por pessoa** e derruba os velhos. Republicar a
fileira a cada abertura do app é seguro: cinco cobre "algumas telas na casa" e
ainda é teto.

⚠️ **Um token de arte vazado não abre filme.** Ele é recusado em `/api/stream/`,
em `/scrub/` e no barramento — conferido, 401 nos três. É esse estreitamento que
paga o ano de validade.

⚠️ **O que mata os dois:** trocar a senha. `POST /api/auth/password` apaga todos
os tokens da conta, dos dois escopos — é o único gesto que revoga o de arte antes
do ano.

---

## O estado deste repositório, medido em 04/08/2026

| | |
|---|---|
| `docs/DESIGN.md` | **8.210 linhas**, última seção **§70** |
| migrações | **37**; a próxima é a `0038` |
| obras | **17.930** · 3 usuários · 3 discos (`/media`, `/media2`, `/media3`) |
| com pôster e `dominant_color` | **9.332** (48% do acervo **não** tem) |
| identificação | `auto` 4.655 · `unmatched` 4.415 · `confirmed` 4.276 · `needs_review` 3.350 · `ignored` 1.234 |
| testes | 257 |

*(Defeito antigo do `DESIGN.md`, achado e não consertado: os números **§12 e §13
aparecem duas vezes**.)*

---

## Armadilhas deste repositório

**⚠️ As migrações são embutidas no binário em tempo de compilação**
(`sqlx::migrate!`). Editar um `.sql` **não** dispara recompilação: o servidor
aplica a versão velha, o checksum diverge, e ele não sobe mais.

```bash
docker exec odeon-api sh -c 'cd /app && touch src/main.rs && cargo build'
docker restart odeon-api
```

**⚠️ Reiniciar o `odeon-api` mata trabalho em andamento.** A identificação roda
em processo; um `docker restart` a derruba no meio, e ela leva ~1h.

**⚠️ O banco tem dados reais de três pessoas.** Empréstimo, nota, resenha, post,
mensagem, conquista e enfeite criados pra testar **devem sair depois**. O padrão
que funciona é conta descartável (`r47teste`) e `DELETE FROM app_user WHERE
username = '…'` no fim — o cascade limpa o resto.

**⚠️ Este repositório é público.** Um backup `.env.antes-do-SAM` já entrou num
`git add` a caminho do push, com a chave do TMDB, a do Groq e a senha do
Postgres dentro. Foi pego no crivo. O `.gitignore` virou `.env*` com
`!.env.example` — **e ainda assim, confira o que vai no commit.**

---

## Como rodar as coisas daqui

`cargo` e `npx` **não** estão no PATH do host. Tudo em container:

```bash
docker exec odeon-api sh -c 'cd /app && cargo test'          # 257 testes
docker exec odeon-db psql -U odeon -d odeon -c "SELECT ..."
```

---

## O que o app vai precisar do servidor nas próximas fases

Nada de novo, e isso é de propósito — a espec do app (§4b, §6) escolheu só o que
o servidor **já dá**. Mas vale saber o que morde:

**A escassez (§66 → §71).** ⚠️ **Isto mudou em 04/08/2026.** Assistir **não**
exige mais empréstimo pro morador: a biblioteca é modo livre, e a exigência
ficou sendo regra da locadora. O `pode_assistir` voltou ao que era antes da R50,
e o `guest` não mudou. Ver §71.

A regra anterior dizia que o app tinha que perguntar
`/api/locadora/liberadas` antes de desenhar o play, ou a tela ofereceria um 403.
Não é mais assim — e foi justamente o app Android, tentando tocar de fora da
locadora, que encontrou o defeito.

A rota continua existindo e continua verdadeira: ela diz quais obras os
empréstimos de alguém cobrem. Serve à locadora, não ao player.

**O token de mídia (§43).** Curto (8h), vai em `?token=` porque `<img>` e o
player não mandam header. **Emitir um novo aposenta o anterior** — e o anterior
pode estar dentro de um player tocando, ou de um Chromecast.

**O barramento (§62).** `/api/events` é SSE com o token na query. **Uma conexão
pro app inteiro.**

**A negociação de reprodução (§M6).** `/api/playback/{id}/plan` recebe
`video_codecs` e `audio_codecs` **do cliente**, e assume que quem pergunta é quem
toca. Com Cast (fase 4) não é: quem toca é o Chromecast. A rota já aceita a lista
como parâmetro, então o conserto é o app mandar o perfil do aparelho de Cast —
**nenhuma linha de servidor**.

**O CORS, se o Cast chegar.** É por regra de mesmo host, e a origem do Chromecast
não é o host do servidor. Provavelmente vai precisar de `ODEON_ALLOWED_ORIGINS`.
