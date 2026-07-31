-- M2 — o grafo.
--
-- O modelo já era um grafo desde o 0001; aqui ele ganha as arestas de uso e os
-- índices pra consultá-lo rápido.
--
-- Correção de rumo: "ordem Machete de Star Wars" NÃO é uma cadeia de
-- `work_edge(watch_order)`. Ordenação linear em grafo de arestas exige CTE
-- recursiva só pra ler a lista, e inserir no meio vira remendo. Ordem de
-- exibição é COLEÇÃO ORDENADA (`collection.kind = 'watch_order'` +
-- `collection_item.position`). As arestas ficam pra relação semântica de par:
-- "este é o corte do diretor DAQUELE", "este é sequência DAQUELE".

-- O CHECK inline do 0001 ganhou nome automático; descobre antes de trocar.
DO $$
DECLARE constraint_name text;
BEGIN
    SELECT conname INTO constraint_name
    FROM pg_constraint
    WHERE conrelid = 'collection'::regclass
      AND contype = 'c'
      AND pg_get_constraintdef(oid) LIKE '%kind%';

    IF constraint_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE collection DROP CONSTRAINT %I', constraint_name);
    END IF;
END $$;

ALTER TABLE collection ADD CONSTRAINT collection_kind_check
    CHECK (kind IN ('series', 'season', 'franchise', 'playlist', 'watch_order', 'custom'));

ALTER TABLE collection ADD COLUMN description text;

-- Coleção criada por provider (série/temporada) não deve ser editável na mão
-- do mesmo jeito que uma playlist minha.
ALTER TABLE collection ADD COLUMN origin text NOT NULL DEFAULT 'manual'
    CHECK (origin IN ('manual', 'provider'));

UPDATE collection SET origin = 'provider' WHERE provider_key IS NOT NULL;

-- Filtro por tag é a consulta mais frequente do M2.
CREATE INDEX work_tag_work_idx  ON work_tag (work_id);
CREATE INDEX tag_namespace_idx  ON tag (namespace);
CREATE INDEX work_edge_to_idx   ON work_edge (to_work);
CREATE INDEX collection_kind_idx ON collection (kind);

-- Ordenação dentro da coleção. Sem isso, "próximo episódio" é table scan.
CREATE INDEX collection_item_position_idx ON collection_item (collection_id, position);

-- Namespaces que a interface trata de forma especial. Não é uma tabela de
-- namespaces permitidos — qualquer namespace novo funciona; estes só ganham
-- cor e posição fixa na UI.
CREATE TABLE tag_namespace (
    namespace   text PRIMARY KEY,
    label       text NOT NULL,
    color       text,
    position    int NOT NULL DEFAULT 100
);

INSERT INTO tag_namespace (namespace, label, color, position) VALUES
    ('format',  'Formato',    '#e0b062', 10),
    ('genre',   'Gênero',     '#7aa2f7', 20),
    ('mood',    'Humor',      '#bb9af7', 30),
    ('vibe',    'Vibe',       '#9ece6a', 40),
    ('origin',  'Origem',     '#7dcfff', 50)
ON CONFLICT (namespace) DO NOTHING;
