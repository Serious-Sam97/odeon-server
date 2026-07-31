-- M5 — curadoria.
--
-- Aqui o `play_event`, guardado cru desde o M0, finalmente paga. A pergunta que
-- o Odeon quer responder não é "o que existe na biblioteca" — é "o que eu
-- assisto AGORA, com o tempo e o humor que eu tenho".
--
-- Duas fontes de sinal, deliberadamente separadas:
--
--  1. COMPORTAMENTO (`play_event`): o que você termina, o que larga aos 10
--     minutos, o que reassiste, a que horas assiste. É o sinal forte.
--  2. CONTEÚDO (`embedding`): sobre o que a obra é. Serve pra "parecido com o
--     que você gostou" e pra dar sugestão a obra que você nunca tocou.
--
-- Comportamento sem conteúdo só recomenda o que você já viu. Conteúdo sem
-- comportamento é um buscador. Os dois juntos é curadoria.

CREATE EXTENSION IF NOT EXISTS vector;

-- 256 dimensões: o suficiente pro hashing trick não colidir demais numa
-- biblioteca pessoal, e pequeno o bastante pra caber na linha sem TOAST.
ALTER TABLE work ADD COLUMN embedding vector(256);
ALTER TABLE work ADD COLUMN embedded_at timestamptz;

-- Índice de vizinho aproximado. Só vale a partir de alguns milhares de obras —
-- abaixo disso o Postgres faz varredura e é mais rápido. Fica criado porque
-- criar depois com a tabela cheia é mais caro.
CREATE INDEX work_embedding_idx ON work
    USING hnsw (embedding vector_cosine_ops);

-- IDF do corpus. Guardar em tabela (em vez de recalcular a cada embed) é o que
-- deixa o vetor de UMA obra reprodutível sem reprocessar a biblioteca inteira.
CREATE TABLE corpus_term (
    term            text PRIMARY KEY,
    document_count  int NOT NULL,
    idf             real NOT NULL
);

CREATE TABLE corpus_stats (
    id              int PRIMARY KEY DEFAULT 1 CHECK (id = 1),
    document_count  int NOT NULL DEFAULT 0,
    built_at        timestamptz NOT NULL DEFAULT now()
);

INSERT INTO corpus_stats (id, document_count) VALUES (1, 0);

-- Feedback explícito. O comportamento cobre quase tudo, mas "não me ofereça
-- mais isso" não tem como ser inferido — largar no meio pode ser interrupção.
CREATE TABLE work_feedback (
    user_id     uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    work_id     uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    -- love: quero mais disso | block: nunca mais me ofereça | later: fica pra depois
    verdict     text NOT NULL CHECK (verdict IN ('love', 'block', 'later')),
    created_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, work_id)
);

CREATE INDEX work_feedback_verdict_idx ON work_feedback (user_id, verdict);

-- O `play_event` é consultado por obra o tempo todo na curadoria; sem este
-- índice cada recomendação varre o log inteiro.
CREATE INDEX play_event_work_idx ON play_event (work_id, created_at DESC);
