-- Canais ao vivo (R6).
--
-- O Odeon **sintoniza**, não programa: uma fonte IPTV publica a lista de canais
-- (M3U) e a grade (XMLTV), e daqui pra frente isto é só leitura. Nada aqui toca
-- o grafo — canal não é `work`, e programa não é `collection`. São eixos
-- diferentes: o grafo descreve o que a obra É, e a grade descreve QUANDO algo
-- passa. Misturar os dois faria a tabela `work` carregar horário.

CREATE TABLE channel_source (
    id           uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name         text NOT NULL,
    m3u_url      text NOT NULL,
    -- Opcional: sem XMLTV existem canais, mas não existe grade.
    xmltv_url    text,
    enabled      boolean NOT NULL DEFAULT true,
    last_import_at timestamptz,
    last_error     text,
    created_at   timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE channel (
    id          uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    source_id   uuid NOT NULL REFERENCES channel_source(id) ON DELETE CASCADE,

    -- O `tvg-id` do M3U. É a chave que casa com o `channel=` do XMLTV, e é o
    -- que permite reimportar sem duplicar — mesmo papel do `provider_key` das
    -- pessoas no M1 (§8h).
    provider_key text NOT NULL,

    name        text NOT NULL,
    number      text,
    logo_url    text,
    grupo       text,

    -- Já REESCRITA no import quando o provider publica loopback. O ErsatzTV
    -- anuncia `http://localhost:8409/iptv/channel/1.ts`, e "localhost" dentro do
    -- container do Odeon é o próprio container. Guardar a URL crua faria o
    -- import passar e todo canal falhar no play. Ver `live::reescreve_host`.
    stream_url  text NOT NULL,

    hidden      boolean NOT NULL DEFAULT false,
    position    int,
    updated_at  timestamptz NOT NULL DEFAULT now(),

    UNIQUE (source_id, provider_key)
);

CREATE INDEX channel_source_idx ON channel (source_id) WHERE NOT hidden;

CREATE TABLE programme (
    id          bigserial PRIMARY KEY,
    channel_id  uuid NOT NULL REFERENCES channel(id) ON DELETE CASCADE,
    starts_at   timestamptz NOT NULL,
    ends_at     timestamptz NOT NULL,
    title       text NOT NULL,
    sub_title   text,
    description text,

    -- Gancho, sem lógica ainda: quando a grade apontar para uma obra que existe
    -- na sua biblioteca, "quer ver do começo?" fica destravado. Custa uma coluna
    -- anulável hoje e evita uma migração dolorosa depois.
    work_id     uuid REFERENCES work(id) ON DELETE SET NULL,

    CHECK (ends_at > starts_at)
);

-- A consulta que a tela faz o tempo todo é "o que passa neste canal entre X e
-- Y", e ela é feita para 17 canais de uma vez.
CREATE INDEX programme_canal_inicio_idx ON programme (channel_id, starts_at);
-- E a do "no ar agora", que varre por instante em vez de por canal.
CREATE INDEX programme_janela_idx ON programme (starts_at, ends_at);

-- O import é uma operação longa como varrer e identificar: entra no mesmo
-- registro, com progresso, cancelamento e histórico.
-- Os seis valores do 0011 continuam TODOS aqui: recriar o CHECK só com os que
-- eu lembrava apagaria `embed`, `reparse` e `scope_apply`, e o histórico de job
-- que já existe no banco passaria a violar a própria constraint.
ALTER TABLE job DROP CONSTRAINT IF EXISTS job_kind_check;
ALTER TABLE job ADD CONSTRAINT job_kind_check
    CHECK (kind IN ('scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply',
                    'live_import'));
