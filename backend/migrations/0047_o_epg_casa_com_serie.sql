-- O programa do EPG passa a poder apontar pra uma **coleção** (R68).
--
-- Medido em 18/08/2026, sobre os 21 programas no ar naquele minuto: 6 sem
-- casamento, e a razão é diferente em cada grupo.
--
--     The Walking Dead     série  → 11 obras com esse título, nenhuma com capa
--     Futurama             série  →  9 obras com esse título, todas episódios
--     007 Contra Goldfinger filme →  2 obras, e são o MESMO filme (tmdb 658)
--
-- O EPG de uma série anuncia o **nome da série**, e `programme.work_id` só
-- sabia apontar pra uma obra. Título de série não casa com obra nenhuma — casa
-- com a `collection`, que desde a R63 tem pôster próprio. As duas séries acima
-- existem como `collection(kind='series')`, com capa, e o casamento passava
-- por elas sem enxergá-las.
--
-- ⚠️ Isto **não** substitui o `work_id`: um filme continua apontando pra obra,
-- que é o que permite "ver desde o início" com o arquivo da casa. A coleção é
-- o alvo de quando não há uma obra só pra apontar.
ALTER TABLE programme
    ADD COLUMN collection_id uuid REFERENCES collection(id) ON DELETE SET NULL;

CREATE INDEX programme_collection_idx
    ON programme (collection_id) WHERE collection_id IS NOT NULL;
