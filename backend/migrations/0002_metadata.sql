-- M1 — identidade.
--
-- A ideia central: o Jellyfin decide sozinho e erra em silêncio. Aqui toda
-- tentativa de casamento vira linha em `match_candidate`, com score E os motivos
-- do score. Dá pra auditar por que uma obra foi identificada como foi, e a fila
-- de revisão manual é parte do design, não plano B.

-- 'auto' decide por heurística de anime; os outros forçam um provider.
ALTER TABLE library ADD COLUMN provider_hint text NOT NULL DEFAULT 'auto'
    CHECK (provider_hint IN ('auto', 'tmdb', 'anilist', 'none'));

-- Chave estável pra reencontrar a série/temporada do provider sem duplicar.
-- Ex.: 'tmdb:tv:1396', 'tmdb:tv:1396:s2', 'anilist:21'
ALTER TABLE collection ADD COLUMN provider_key text UNIQUE;

CREATE TABLE match_candidate (
    id             uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    work_id        uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,

    provider       text NOT NULL CHECK (provider IN ('tmdb', 'anilist')),
    provider_id    text NOT NULL,
    provider_kind  text NOT NULL CHECK (provider_kind IN ('movie', 'tv', 'anime')),

    title          text NOT NULL,
    original_title text,
    year           int,
    overview       text,
    poster_url     text,
    backdrop_url   text,
    accent_color   text,
    popularity     real NOT NULL DEFAULT 0,

    score          real NOT NULL,
    -- ["título bate (0.97)", "ano confere: 2017", "provider diz que é filme"]
    reasons        jsonb NOT NULL DEFAULT '[]'::jsonb,
    raw            jsonb,

    created_at     timestamptz NOT NULL DEFAULT now(),
    UNIQUE (work_id, provider, provider_id)
);

CREATE INDEX match_candidate_work_idx ON match_candidate (work_id, score DESC);

-- Qual candidato venceu. NULL = ninguém venceu ainda.
ALTER TABLE work ADD COLUMN matched_candidate_id uuid
    REFERENCES match_candidate(id) ON DELETE SET NULL;

-- Quando a última tentativa rodou — evita re-consultar provider à toa.
ALTER TABLE work ADD COLUMN matched_at timestamptz;

-- Tags que o matcher cria sozinho. 'anime' é TAG, não `kind` — um episódio de
-- anime continua sendo kind='episode'. É exatamente pra isso que o namespace
-- de tags existe.
INSERT INTO tag (namespace, value) VALUES
    ('format', 'anime'),
    ('format', 'série'),
    ('format', 'filme')
ON CONFLICT (namespace, value) DO NOTHING;
