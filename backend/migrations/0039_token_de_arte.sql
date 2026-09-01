-- R45 — o token de arte, longo, pra fileira da home da Google TV.
--
-- ## O que quebrou, e por que não dava pra consertar do lado do app
--
-- A home da Google TV **não busca a imagem pelo app**. O que se entrega ao
-- sistema é uma `Uri`, guardada no `TvProvider`, e quem a baixa é o processo do
-- launcher — dias depois, com o Odeon fechado. É a única superfície do produto
-- em que o cliente entrega uma credencial e perde o controle de quando ela é
-- usada: não há gancho de "a imagem falhou, peça de novo", não há interceptor, e
-- o `OkHttp` da casa nem está no processo que faz a requisição.
--
-- O `/artwork` está atrás do portão e aceita `?token=` (`middleware.rs`), mas o
-- ramo da query resolve **token de mídia** — e a R27 deu a ele oito horas.
--
-- ## O agravante, que é o que torna o paliativo inútil
--
-- O pedido do app supunha "o token roda, e um dia as artes caem". É pior:
--
--     DELETE FROM media_token WHERE user_id = $1 OR expira_em <= now()
--
-- `emitir_token_de_midia` **aposenta os anteriores da mesma pessoa**. Então não
-- é o prazo que derruba a fileira — é abrir o Odeon no celular. O paliativo
-- escrito no `CanalDaHome.kt` (republicar todas as URLs a cada abertura do app)
-- não cobre uma semana: cobre até o próximo play em outro aparelho.
--
-- ## Por que um escopo na tabela, e não uma tabela nova
--
-- O `middleware.rs` já recusou um terceiro tipo de token uma vez, pro
-- barramento, com a frase "três escopos pra isso é mais máquina do que o risco
-- pede". A frase continua valendo: o que muda aqui não é a natureza do token
-- (segredo opaco, guardado como SHA-256, revogável por `DELETE`), é o **prazo**
-- e **o que ele abre**. Uma coluna carrega isso; uma tabela repetiria as quatro
-- colunas, os dois índices e as duas funções pra dizer a mesma coisa.
--
-- ## O que o escopo `arte` abre, dito com todas as letras
--
-- `/artwork/` e nada mais. Não abre `/api/stream/`, não abre `/api/hls/`, não
-- abre `/scrub/` e não abre o barramento — quem trata disso é
-- `aceita_token_de_arte` no middleware, e o escopo é conferido no `SELECT`.
--
-- O que está atrás de `/artwork/` são pôsteres e backdrops **baixados do TMDB**:
-- nenhum byte do acervo, nenhuma credencial. Um token de arte vazado revela que
-- esta casa tem a capa de tal filme. É o risco que o ano de validade compra, e
-- ele é menor que o do token de mídia, que dura oito horas justamente porque
-- entrega o filme inteiro.
--
-- ## O ano
--
-- O prazo tem que sobreviver ao aparelho que **não abre o app**. É esse o caso
-- que o pedido descreve: quem passou um mês sem abrir o Odeon e olha a primeira
-- tela da TV. Um mês é o piso; um ano é a ordem de grandeza de "não pensar
-- nisso de novo", e ainda é finito — a linha vence e some sozinha, que é o que
-- separa isto de deixar `/artwork` aberto.

ALTER TABLE media_token
    ADD COLUMN escopo text NOT NULL DEFAULT 'midia'
        CHECK (escopo IN ('midia', 'arte'));

-- As linhas que já existem são todas de mídia — é o único escopo que havia. O
-- DEFAULT acima já as marca, e ele fica: quem inserir sem dizer o escopo está
-- pedindo o comportamento da R27, que é o restrito.

-- "Os tokens de arte desta pessoa, do mais novo pro mais velho" — é a consulta
-- da poda (ver `emitir_token_de_arte`), e a única que precisa da ordem.
CREATE INDEX media_token_arte_idx
    ON media_token (user_id, criado_em DESC)
    WHERE escopo = 'arte';
