-- R27 — o token de mídia, separado do token de sessão.
--
-- ## A dívida que isto paga, e ela tem nome desde o M6
--
-- O `auth/middleware.rs` aceita o token por três caminhos, e o terceiro sempre
-- foi um compromisso declarado:
--
-- > `?token=` na query — **só nas rotas de mídia**, exatamente porque são as
-- > que um elemento HTML busca sozinho. […] Token em query string vaza pra log
-- > de acesso e histórico do navegador. […] Se um dia isso for exposto de
-- > verdade, o certo é **emitir um token de mídia curto e separado do token de
-- > sessão**.
--
-- O §6.5 listou esse `?token=` como o primeiro dos três compromissos que "gente
-- de fora" cobraria, e a R26 (§42) trouxe gente de fora sem pagá-lo. Esta
-- migração paga.
--
-- ## O que muda de verdade
--
-- Hoje o que vai na query **é o token de sessão**: 90 dias de validade, acesso
-- total à API. Um `access.log` de proxy, um histórico de navegador ou um print
-- de tela com a URL do vídeo entrega uma sessão inteira.
--
-- Depois desta migração, o que vai na query é um token que **só abre mídia** e
-- **vence em horas**. A rota de API continua exigindo `Authorization` — um
-- token de mídia não serve pra listar biblioteca, nem pra alugar, nem pra nada.
--
-- ## Por que uma tabela, e não um HMAC assinado
--
-- Um token assinado dispensaria a tabela e a consulta. O §9b já enfrentou
-- exatamente essa escolha ao recusar JWT para a sessão:
--
-- > JWT é stateless, o que soa bom até você querer deslogar um aparelho
-- > perdido.
--
-- Vale o mesmo aqui, e vale mais: um token de mídia vazado é a coisa que se
-- quer revogar **agora**, não daqui a oito horas. A linha some da tabela e
-- acabou. E a consulta é um `SELECT` por chave primária numa rota que já lê o
-- banco pra achar o arquivo.
--
-- Guardamos o SHA-256, como `auth_session` e `convite`. Vazar o banco não dá
-- mídia a ninguém.
--
-- ## As oito horas, medidas
--
-- O prazo tem que sobreviver a assistir a coisa mais longa do acervo sem
-- interrupção. Medido: o arquivo mais longo tem **4,9 h**, o filme mais longo
-- **4,04 h** (a Liga da Justiça do Snyder), 15 arquivos passam de 3 h e
-- **nenhum passa de 5 h**.
--
-- Oito horas cobrem o maior arquivo com três horas de pausa por cima — e são
-- 1/270 da validade do token de sessão. Um número menor quebraria a reprodução
-- no meio; um maior desfaria a razão de a fase existir.

CREATE TABLE media_token (
    -- SHA-256 do token, em hex. O token em si nunca toca o disco.
    token_hash text PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    criado_em  timestamptz NOT NULL DEFAULT now(),
    expira_em  timestamptz NOT NULL
);

-- "Os tokens deste usuário" — pra emitir um novo poder aposentar os velhos, e
-- pra que revogar as sessões de alguém possa revogar a mídia junto.
CREATE INDEX media_token_user_idx ON media_token (user_id);

-- A varredura de expirados, que roda junto da limpeza de sessão.
CREATE INDEX media_token_expira_idx ON media_token (expira_em);
