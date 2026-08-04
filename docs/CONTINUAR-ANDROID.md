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

**A escassez (§66).** Assistir exige empréstimo em aberto, **inclusive pro
admin**. O app tem que perguntar `/api/locadora/liberadas` **antes** de desenhar
o botão de play, ou a tela oferece um 403. Isso entra na fase 2.

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
