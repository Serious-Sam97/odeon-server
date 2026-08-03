-- R46 — assistir junto.
--
-- Três tabelas, e a divisão é a das três perguntas que o `IDEIAS-2.md` §4.6
-- respondeu: **quem manda** (o host, na sessão), **quem está** (os membros, com
-- o estado de carregamento de cada um) e **o que foi dito** (a conversa, que
-- fica guardada).
--
-- ## Por que a sessão guarda o estado, e não só o barramento
--
-- O barramento do M3 entrega o evento a quem está ouvindo **agora**. Quem entra
-- trinta segundos depois precisa saber onde o filme está — e é o mesmo defeito
-- que a R44 encontrou no aviso de programa: um evento publicado no vazio some.
-- Então a posição e o "tocando" são estado, e o evento só avisa que mudou.
CREATE TABLE sessao_junta (
    id                    uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    work_id               uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    media_file_id         uuid REFERENCES media_file(id) ON DELETE SET NULL,
    host_id               uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,

    -- **Os dois modos existem, como opção da sessão** (§4.6). O padrão é um
    -- stream por pessoa: é o que já funciona sem código novo, e o compartilhado
    -- é a otimização que se liga quando a máquina reclamar.
    modo                  text NOT NULL DEFAULT 'por_pessoa'
                          CHECK (modo IN ('por_pessoa', 'compartilhado')),
    -- No modo compartilhado: a sessão de transcode do host, que os outros leem
    -- em vez de abrir a sua. `NULL` no modo por pessoa.
    transcode_id          uuid,

    -- A INTENÇÃO do host. O que toca de verdade é isto **e** todo mundo pronto
    -- — ver `sessao_junta_membro.pronto`.
    tocando               boolean NOT NULL DEFAULT false,
    posicao_segundos      double precision NOT NULL DEFAULT 0,

    criada_em             timestamptz NOT NULL DEFAULT now(),
    atualizado_em         timestamptz NOT NULL DEFAULT now(),
    encerrada_em          timestamptz
);

-- Uma sessão aberta por host. Duas seriam duas salas do mesmo dono, e ninguém
-- saberia em qual entrar.
CREATE UNIQUE INDEX sessao_junta_uma_por_host
    ON sessao_junta (host_id) WHERE encerrada_em IS NULL;

CREATE INDEX sessao_junta_abertas ON sessao_junta (criada_em DESC)
    WHERE encerrada_em IS NULL;

CREATE TABLE sessao_junta_membro (
    sessao_id  uuid NOT NULL REFERENCES sessao_junta(id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    entrou_em  timestamptz NOT NULL DEFAULT now(),
    -- **Carregado e pronto pra tocar.** É o campo que faz "quando um trava,
    -- todo mundo para" ser um fato do servidor e não um acordo entre telas.
    pronto     boolean NOT NULL DEFAULT false,
    visto_em   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (sessao_id, user_id)
);

CREATE TABLE sessao_junta_recado (
    id         bigserial PRIMARY KEY,
    sessao_id  uuid NOT NULL REFERENCES sessao_junta(id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    texto      text NOT NULL CHECK (length(texto) BETWEEN 1 AND 500),
    em         timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX sessao_junta_recado_sala ON sessao_junta_recado (sessao_id, em);
