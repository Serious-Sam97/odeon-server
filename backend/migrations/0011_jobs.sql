-- Estado das operações longas, PERSISTIDO.
--
-- POR QUE ESTA TABELA EXISTE
-- Varredura, identificação, sprites e embeddings viviam em
-- `Arc<Mutex<Status>>` no processo. Consequências que aconteceram de verdade
-- durante a implantação deste servidor:
--
--   - um `systemctl stop docker` matou uma varredura de 17 mil arquivos no
--     meio, e depois do restart o status dizia `running: false` — indistinguível
--     de "nunca rodou";
--   - uma reaplicação de escopo de 16 minutos morreu porque o `cargo watch`
--     reiniciou o processo, e não havia registro de que 59 de 500 tinham sido
--     aplicadas;
--   - não havia como cancelar nada: parar uma execução exigia matar o processo.
--
-- O estado aqui é DERIVADO das mesmas structs que já existiam — o formato JSON
-- dos endpoints `/status` continua idêntico, porque há quatro alvos de cliente
-- (web, Android, TV, iOS) e quebrar todos por causa disto não se justifica.
CREATE TABLE job (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),

    kind          text NOT NULL CHECK (kind IN
                  ('scan', 'match', 'scrub', 'embed', 'reparse', 'scope_apply')),

    -- `interrupted` é o estado que não existia e fazia falta: o processo morreu
    -- durante a execução. Diferente de `failed` (a operação deu erro) e de
    -- `cancelled` (alguém pediu pra parar).
    state         text NOT NULL DEFAULT 'running' CHECK (state IN
                  ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'interrupted')),

    params        jsonb NOT NULL DEFAULT '{}'::jsonb,
    -- O `Status` de hoje, inteiro. Guardar o formato existente em vez de
    -- normalizar em colunas é o que permite os `/status` continuarem iguais.
    progress      jsonb NOT NULL DEFAULT '{}'::jsonb,

    total         int,
    done          int NOT NULL DEFAULT 0,
    failed        int NOT NULL DEFAULT 0,
    current       text,

    -- Mesma regra do M1: toda decisão guarda frases legíveis.
    reasons       jsonb NOT NULL DEFAULT '[]'::jsonb,
    error         text,

    -- Cancelamento é COOPERATIVO: quem pede só marca aqui, e o worker decide
    -- onde é seguro parar. Matar no meio de uma transação seria pior que
    -- esperar o item terminar.
    cancel_requested boolean NOT NULL DEFAULT false,

    requested_by  uuid REFERENCES app_user(id) ON DELETE SET NULL,
    started_at    timestamptz NOT NULL DEFAULT now(),
    finished_at   timestamptz,
    heartbeat_at  timestamptz NOT NULL DEFAULT now()
);

-- Uma operação de cada tipo por vez. Substitui a flag `running` que cada módulo
-- checava por conta própria — agora a garantia é do banco, não de convenção.
CREATE UNIQUE INDEX job_one_active_per_kind ON job (kind)
    WHERE state IN ('queued', 'running');

-- O histórico é lido "o mais recente deste tipo" o tempo todo.
CREATE INDEX job_recent_idx ON job (kind, started_at DESC);
