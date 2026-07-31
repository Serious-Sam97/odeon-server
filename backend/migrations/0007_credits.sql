-- Elenco e equipe.
--
-- As tabelas `person` e `credit` existem desde o 0001 — o modelo já as previa,
-- só ninguém as preenchia. Aqui elas ganham o que faltava pra serem úteis:
--
--  * `provider_key` pra deduplicar. Sem isso, "Denis Villeneuve" viraria uma
--    linha nova a cada filme dele, e "tudo do Villeneuve" devolveria um filme.
--  * `image_path` pro retrato ficar em cache local, igual ao artwork do M1 —
--    a biblioteca continua funcionando com a internet fora.

ALTER TABLE person ADD COLUMN provider_key text UNIQUE;
ALTER TABLE person ADD COLUMN image_path text;
-- "Directing", "Acting", "Sound"… o que a pessoa faz predominantemente.
ALTER TABLE person ADD COLUMN known_for text;
ALTER TABLE person ADD COLUMN updated_at timestamptz NOT NULL DEFAULT now();

-- "Todos os trabalhos desta pessoa" é a consulta que justifica a tabela.
CREATE INDEX credit_person_idx ON credit (person_id, role);
CREATE INDEX person_provider_idx ON person (provider_key);

-- O CHECK de papéis fica fora de propósito: `credit.role` é texto livre porque
-- provider inventa cargo o tempo todo ("Original Creator", "Key Animation").
-- O que a interface destaca é decidido aqui, não no schema.
CREATE TABLE credit_role (
    role     text PRIMARY KEY,
    label    text NOT NULL,
    -- Papéis de destaque aparecem na tela de detalhe; o resto fica no "mais".
    featured boolean NOT NULL DEFAULT false,
    position int NOT NULL DEFAULT 100
);

INSERT INTO credit_role (role, label, featured, position) VALUES
    ('director',   'Direção',      true,  10),
    ('creator',    'Criação',      true,  20),
    ('writer',     'Roteiro',      true,  30),
    ('actor',      'Elenco',       true,  40),
    ('composer',   'Trilha',       false, 50),
    ('voice',      'Vozes',        true,  45),
    ('producer',   'Produção',     false, 60),
    ('animation',  'Animação',     false, 70)
ON CONFLICT (role) DO NOTHING;
