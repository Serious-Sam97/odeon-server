-- Odeon — schema base.
--
-- Princípio: NÃO existe tabela "movie" ou "tv_show". Existe `work` (a obra) e
-- `collection` (agrupamento recursivo), ligados por `edge`/`collection_item`.
-- Um filme é um work sem coleção. Um episódio é um work dentro de uma season
-- dentro de uma series. Um especial de stand-up é um work solto. Uma franquia é
-- uma collection de collections. Nada disso precisa de tabela nova.
--
-- Tipos são TEXT + CHECK, não ENUM: enum do Postgres é doloroso de evoluir e
-- este modelo vai mudar muito nos próximos meses.

CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- ---------------------------------------------------------------- bibliotecas

CREATE TABLE library (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name          text NOT NULL,
    root_path     text NOT NULL UNIQUE,
    default_kind  text NOT NULL DEFAULT 'movie',
    created_at    timestamptz NOT NULL DEFAULT now()
);

-- ---------------------------------------------------------------------- obras

CREATE TABLE work (
    id               uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind             text NOT NULL DEFAULT 'unknown'
                     CHECK (kind IN ('movie','episode','short','standup','concert',
                                     'documentary','music_video','other','unknown')),
    title            text NOT NULL,
    original_title   text,
    year             int,
    overview         text,
    runtime_seconds  int,

    -- posição serial, quando faz sentido (episódio)
    season_number    int,
    episode_number   int,

    -- identificação externa (M1): {"tmdb": 603, "anilist": 21, "imdb": "tt0133093"}
    external_ids     jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- unmatched: ninguém tentou | auto: casado com confiança
    -- needs_review: casado com dúvida, precisa de humano | confirmed: humano confirmou
    match_state      text NOT NULL DEFAULT 'unmatched'
                     CHECK (match_state IN ('unmatched','auto','needs_review','confirmed')),
    match_confidence real,

    artwork          jsonb NOT NULL DEFAULT '{}'::jsonb,
    dominant_color   text,

    created_at       timestamptz NOT NULL DEFAULT now(),
    updated_at       timestamptz NOT NULL DEFAULT now(),

    search_vector    tsvector GENERATED ALWAYS AS (
        to_tsvector('simple',
            coalesce(title, '') || ' ' ||
            coalesce(original_title, '') || ' ' ||
            coalesce(overview, ''))
    ) STORED
);

CREATE INDEX work_search_idx     ON work USING gin (search_vector);
CREATE INDEX work_title_trgm_idx ON work USING gin (title gin_trgm_ops);
CREATE INDEX work_match_state_idx ON work (match_state) WHERE match_state <> 'confirmed';

-- ------------------------------------------------------------------ coleções

CREATE TABLE collection (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    kind        text NOT NULL
                CHECK (kind IN ('series','season','franchise','playlist','custom')),
    parent_id   uuid REFERENCES collection(id) ON DELETE CASCADE,
    title       text NOT NULL,
    year        int,
    overview    text,
    position    int,
    external_ids jsonb NOT NULL DEFAULT '{}'::jsonb,
    artwork     jsonb NOT NULL DEFAULT '{}'::jsonb,
    created_at  timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX collection_parent_idx ON collection (parent_id);

CREATE TABLE collection_item (
    collection_id uuid NOT NULL REFERENCES collection(id) ON DELETE CASCADE,
    work_id       uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    position      int,
    PRIMARY KEY (collection_id, work_id)
);

-- Relação obra↔obra. É isso que permite "ordem Machete de Star Wars" sem gambiarra.
CREATE TABLE work_edge (
    from_work uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    to_work   uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    kind      text NOT NULL
              CHECK (kind IN ('sequel_of','prequel_of','remake_of','alternate_cut_of',
                              'watch_order','related')),
    label     text,
    position  int,
    PRIMARY KEY (from_work, to_work, kind),
    CHECK (from_work <> to_work)
);

-- ------------------------------------------------------------------ taxonomia

-- Tags com namespace: ('genre','ficção científica'), ('mood','melancólico'),
-- ('format','anime'), ('vibe','pra assistir sozinho de madrugada').
CREATE TABLE tag (
    id        uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    namespace text NOT NULL,
    value     text NOT NULL,
    color     text,
    UNIQUE (namespace, value)
);

CREATE TABLE work_tag (
    work_id uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    tag_id  uuid NOT NULL REFERENCES tag(id) ON DELETE CASCADE,
    weight  real NOT NULL DEFAULT 1.0,
    source  text NOT NULL DEFAULT 'manual'
            CHECK (source IN ('manual','provider','inferred')),
    PRIMARY KEY (work_id, tag_id)
);

CREATE INDEX work_tag_tag_idx ON work_tag (tag_id);

CREATE TABLE person (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text NOT NULL,
    external_ids jsonb NOT NULL DEFAULT '{}'::jsonb,
    image_url    text
);

CREATE INDEX person_name_trgm_idx ON person USING gin (name gin_trgm_ops);

CREATE TABLE credit (
    work_id        uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    person_id      uuid NOT NULL REFERENCES person(id) ON DELETE CASCADE,
    role           text NOT NULL,   -- director, writer, actor, composer, ...
    character_name text,
    position       int,
    PRIMARY KEY (work_id, person_id, role)
);

-- -------------------------------------------------------------- arquivos

CREATE TABLE media_file (
    id              uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    library_id      uuid NOT NULL REFERENCES library(id) ON DELETE CASCADE,
    work_id         uuid REFERENCES work(id) ON DELETE SET NULL,

    path            text NOT NULL UNIQUE,
    filename        text NOT NULL,
    size_bytes      bigint NOT NULL,
    mtime           timestamptz NOT NULL,

    container       text,
    duration_seconds double precision,
    bitrate          bigint,
    video_codec      text,
    width            int,
    height           int,
    frame_rate       double precision,
    audio_codec      text,
    audio_channels   int,
    subtitle_langs   text[] NOT NULL DEFAULT '{}',
    probe            jsonb,

    -- discovered: achado no disco | probed: ffprobe rodou | missing: sumiu do disco
    status          text NOT NULL DEFAULT 'discovered'
                    CHECK (status IN ('discovered','probed','error','missing')),
    error_message   text,
    scanned_at      timestamptz NOT NULL DEFAULT now(),
    created_at      timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX media_file_library_idx ON media_file (library_id);
CREATE INDEX media_file_work_idx    ON media_file (work_id);

-- ------------------------------------------------------------- usuários e play

CREATE TABLE app_user (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    username     text NOT NULL UNIQUE,
    display_name text NOT NULL,
    created_at   timestamptz NOT NULL DEFAULT now()
);

-- O LOG. Fonte da verdade. Não é um booleano "assistido" — é o histórico cru,
-- e é o que vai alimentar a curadoria lá no M5.
CREATE TABLE play_event (
    id               bigserial PRIMARY KEY,
    user_id          uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    work_id          uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    media_file_id    uuid REFERENCES media_file(id) ON DELETE SET NULL,
    event_type       text NOT NULL
                     CHECK (event_type IN ('start','progress','pause','seek','finish','abandon')),
    position_seconds double precision NOT NULL,
    duration_seconds double precision,
    client           text,
    created_at       timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX play_event_user_work_idx ON play_event (user_id, work_id, created_at DESC);

-- Cache derivado do log acima. Existe só pra "continuar assistindo" ser rápido.
CREATE TABLE playback_state (
    user_id          uuid NOT NULL REFERENCES app_user(id) ON DELETE CASCADE,
    work_id          uuid NOT NULL REFERENCES work(id) ON DELETE CASCADE,
    position_seconds double precision NOT NULL DEFAULT 0,
    duration_seconds double precision,
    finished         boolean NOT NULL DEFAULT false,
    play_count       int NOT NULL DEFAULT 0,
    updated_at       timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, work_id)
);

CREATE INDEX playback_state_recent_idx
    ON playback_state (user_id, updated_at DESC) WHERE NOT finished;

-- ---------------------------------------------------------------------- seed

INSERT INTO app_user (username, display_name) VALUES ('sam', 'Sam');
